// Release builds on Windows are GUI apps, so no console window opens beside them.
#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

mod app;
mod auto_intel;
mod chain_archive;
mod config;
mod data;
mod esi;
mod forms;
mod local_intel;
mod model;
mod paths;
mod portrait;
mod recon;
mod store;
mod util;
mod widgets;
mod zkill;

use app::NomadeerApp;
use eframe::egui;

/// Matches the desktop entry and AppStream metainfo under packaging/linux, so
/// desktops group the window with its launcher.
const APP_ID: &str = "io.github.TomD3Wiz.Nomadeer";

const ICON_PNG: &[u8] = include_bytes!("../assets/icons/nomadeer-256.png");

/// Run by the Windows installer on the compatible build to pick an edition: exits
/// with 0 when this processor can run the optimized build and 1 when it cannot.
const SUPPORTS_OPTIMIZED_FLAG: &str = "--supports-optimized-edition";

fn main() -> eframe::Result<()> {
    // Already run before startup on Linux and Windows; this covers other systems.
    ensure_cpu_supports_build();
    if std::env::args_os()
        .nth(1)
        .is_some_and(|arg| arg == SUPPORTS_OPTIMIZED_FLAG)
    {
        std::process::exit(if cpu::supports_optimized_build() {
            0
        } else {
            1
        });
    }
    // SAFETY: nothing has started another thread yet.
    unsafe { local_intel::limit_cpu_threads() };
    #[cfg(windows)]
    show_main_thread_panics();

    let mut viewport = egui::ViewportBuilder::default()
        .with_title("Nomadeer")
        .with_app_id(APP_ID)
        .with_inner_size([1280.0, 820.0])
        .with_min_inner_size([960.0, 640.0]);
    if let Some(icon) = app_icon() {
        viewport = viewport.with_icon(icon);
    }

    let options = eframe::NativeOptions {
        viewport,
        ..Default::default()
    };

    let result = eframe::run_native(
        "Nomadeer",
        options,
        Box::new(|cc| Ok(Box::new(NomadeerApp::new(cc)))),
    );
    // Release builds on Windows have no console, where Rust would print the error,
    // so it would otherwise vanish with the process.
    #[cfg(windows)]
    if let Err(error) = &result {
        windows_dialog::show_error_text(&fatal_error_message(&error.to_string()));
    }
    result
}

#[cfg(any(windows, test))]
fn fatal_error_message(error: &str) -> String {
    format!(
        "Nomadeer ran into a problem and has to close:\n\n{error}\n\n\
         If the window never opened, updating your graphics drivers may help."
    )
}

/// Shows a panic on the main thread (which ends the app) in a dialog. Panics in
/// background workers are left to the workers, which recover from them.
#[cfg(windows)]
fn show_main_thread_panics() {
    let default_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        default_hook(info);
        if std::thread::current().name() == Some("main") {
            windows_dialog::show_error_text(&fatal_error_message(&info.to_string()));
        }
    }));
}

fn app_icon() -> Option<egui::IconData> {
    let image = image::load_from_memory(ICON_PNG).ok()?.into_rgba8();
    let (width, height) = image.dimensions();
    Some(egui::IconData {
        rgba: image.into_raw(),
        width,
        height,
    })
}

/// Shown when the optimized build is started on a CPU it cannot run on.
#[cfg(any(all(target_arch = "x86_64", target_feature = "avx2"), test))]
const CPU_UNSUPPORTED_MESSAGE: &str = "This optimized Nomadeer build needs a newer CPU \
     (x86-64-v3, with AVX2). Please use the compatible build instead.";

/// The optimized release is compiled for x86-64-v3 CPUs. Exit with a clear message
/// on older CPUs instead of crashing on the first unsupported instruction.
///
/// Rust's own startup code is built for x86-64-v3 too and already uses AVX before
/// `main`, so on Linux and Windows this runs earlier, as a C runtime initializer
/// (see `CHECK_CPU_BEFORE_STARTUP`). It therefore reports through the C library or
/// Windows directly instead of std.
#[cfg(all(target_arch = "x86_64", target_feature = "avx2"))]
extern "C" fn ensure_cpu_supports_build() {
    if cpu::supports_optimized_build() {
        return;
    }
    // Windows release builds have no console, so the player would see nothing. The
    // text is encoded at compile time to run as little AVX-built code as possible
    // on a CPU without it.
    #[cfg(windows)]
    {
        const TITLE: [u16; 9] = windows_dialog::wide("Nomadeer");
        const TEXT: [u16; CPU_UNSUPPORTED_MESSAGE.len() + 1] =
            windows_dialog::wide(CPU_UNSUPPORTED_MESSAGE);
        windows_dialog::show_error(&TITLE, &TEXT);

        #[link(name = "kernel32")]
        unsafe extern "system" {
            fn ExitProcess(code: u32) -> !;
        }
        // SAFETY: ends the process; nothing has started that needs cleaning up.
        unsafe { ExitProcess(1) }
    }
    #[cfg(not(windows))]
    {
        unsafe extern "C" {
            fn write(fd: i32, buf: *const std::ffi::c_void, count: usize) -> isize;
            fn _exit(status: i32) -> !;
        }
        let line = unsupported_cpu_line();
        // SAFETY: writes a static buffer to stderr, then ends the process without
        // running anything else.
        unsafe {
            write(2, line.as_ptr().cast(), line.len());
            _exit(1)
        }
    }
}

/// `CPU_UNSUPPORTED_MESSAGE` and a newline, built at compile time.
#[cfg(all(target_arch = "x86_64", target_feature = "avx2", not(windows)))]
const fn unsupported_cpu_line() -> &'static [u8] {
    const LEN: usize = CPU_UNSUPPORTED_MESSAGE.len() + 1;
    const LINE: [u8; LEN] = {
        let message = CPU_UNSUPPORTED_MESSAGE.as_bytes();
        let mut line = [b'\n'; LEN];
        let mut index = 0;
        while index < message.len() {
            line[index] = message[index];
            index += 1;
        }
        line
    };
    &LINE
}

/// Runs the processor check before Rust's startup code, from the C runtime's list
/// of initializers.
#[cfg(all(
    target_arch = "x86_64",
    target_feature = "avx2",
    any(target_os = "linux", windows)
))]
#[used]
#[cfg_attr(target_os = "linux", unsafe(link_section = ".init_array"))]
#[cfg_attr(windows, unsafe(link_section = ".CRT$XCU"))]
static CHECK_CPU_BEFORE_STARTUP: extern "C" fn() = ensure_cpu_supports_build;

#[cfg(not(all(target_arch = "x86_64", target_feature = "avx2")))]
fn ensure_cpu_supports_build() {}

/// Processor checks for the optimized (x86-64-v3) build.
///
/// These read CPUID directly. `is_x86_feature_detected!` answers from the compile
/// time target features first, so inside the optimized build it is always true and
/// would wave every processor through.
mod cpu {
    /// The CPUID and XCR0 bits the x86-64-v3 check looks at.
    #[derive(Clone, Copy, Default)]
    pub(crate) struct Registers {
        /// Leaf 1, ECX.
        pub(crate) leaf1_ecx: u32,
        /// Leaf 7 sub-leaf 0, EBX.
        pub(crate) leaf7_ebx: u32,
        /// Leaf 0x8000_0001, ECX.
        pub(crate) ext1_ecx: u32,
        /// XCR0, only readable when the OS has enabled XSAVE.
        pub(crate) xcr0: u64,
    }

    pub(crate) const FMA: u32 = 1 << 12;
    pub(crate) const MOVBE: u32 = 1 << 22;
    pub(crate) const OSXSAVE: u32 = 1 << 27;
    pub(crate) const AVX: u32 = 1 << 28;
    pub(crate) const F16C: u32 = 1 << 29;
    pub(crate) const BMI1: u32 = 1 << 3;
    pub(crate) const AVX2: u32 = 1 << 5;
    pub(crate) const BMI2: u32 = 1 << 8;
    /// LZCNT, listed as `abm` in /proc/cpuinfo.
    pub(crate) const LZCNT: u32 = 1 << 5;
    /// The OS saves SSE and AVX register state across task switches.
    pub(crate) const XCR0_SSE_AVX: u64 = 0b110;

    /// The same feature set the Linux launcher requires (avx avx2 bmi1 bmi2 f16c
    /// fma movbe abm), plus OS support for the AVX registers.
    pub(crate) fn supports_x86_64_v3(registers: Registers) -> bool {
        let has = |value: u32, bits: u32| value & bits == bits;
        has(registers.leaf1_ecx, FMA | MOVBE | OSXSAVE | AVX | F16C)
            && has(registers.leaf7_ebx, BMI1 | AVX2 | BMI2)
            && has(registers.ext1_ecx, LZCNT)
            && registers.xcr0 & XCR0_SSE_AVX == XCR0_SSE_AVX
    }

    #[cfg(target_arch = "x86_64")]
    pub(crate) fn supports_optimized_build() -> bool {
        supports_x86_64_v3(read_registers())
    }

    #[cfg(not(target_arch = "x86_64"))]
    pub(crate) fn supports_optimized_build() -> bool {
        false
    }

    #[cfg(target_arch = "x86_64")]
    fn read_registers() -> Registers {
        use std::arch::x86_64::{__cpuid, __cpuid_count};

        let max_leaf = __cpuid(0).eax;
        let max_extended_leaf = __cpuid(0x8000_0000).eax;
        let leaf1_ecx = if max_leaf >= 1 { __cpuid(1).ecx } else { 0 };
        let xcr0 = if leaf1_ecx & OSXSAVE == 0 {
            0
        } else {
            let (low, high): (u32, u32);
            // SAFETY: OSXSAVE says the OS has enabled XGETBV, and reading XCR0
            // touches no memory.
            unsafe {
                std::arch::asm!(
                    "xgetbv",
                    in("ecx") 0,
                    out("eax") low,
                    out("edx") high,
                    options(nomem, nostack, preserves_flags),
                );
            }
            (u64::from(high) << 32) | u64::from(low)
        };
        Registers {
            leaf1_ecx,
            leaf7_ebx: if max_leaf >= 7 {
                __cpuid_count(7, 0).ebx
            } else {
                0
            },
            ext1_ecx: if max_extended_leaf >= 0x8000_0001 {
                __cpuid(0x8000_0001).ecx
            } else {
                0
            },
            xcr0,
        }
    }
}

/// A native error dialog for problems that stop Nomadeer before its window opens.
#[cfg(any(windows, test))]
#[allow(dead_code)]
mod windows_dialog {
    /// NUL-terminated UTF-16 for an ASCII `text`; `N` must be its length plus one.
    pub(crate) const fn wide<const N: usize>(text: &str) -> [u16; N] {
        let bytes = text.as_bytes();
        assert!(bytes.len() + 1 == N, "N must leave room for the NUL");
        let mut wide = [0_u16; N];
        let mut index = 0;
        while index < bytes.len() {
            assert!(bytes[index].is_ascii(), "only ASCII text is supported");
            wide[index] = bytes[index] as u16;
            index += 1;
        }
        wide
    }

    /// NUL-terminated UTF-16 for any `text`, for messages built at run time.
    pub(crate) fn to_wide(text: &str) -> Vec<u16> {
        text.encode_utf16().chain(Some(0)).collect()
    }

    #[cfg(windows)]
    pub(crate) fn show_error_text(text: &str) {
        show_error(&wide::<9>("Nomadeer"), &to_wide(text));
    }

    #[cfg(windows)]
    pub(crate) fn show_error(title: &[u16], text: &[u16]) {
        use std::ffi::c_void;

        const MB_OK: u32 = 0x0000_0000;
        const MB_ICONERROR: u32 = 0x0000_0010;

        #[link(name = "user32")]
        unsafe extern "system" {
            fn MessageBoxW(
                hwnd: *mut c_void,
                text: *const u16,
                caption: *const u16,
                kind: u32,
            ) -> i32;
        }

        assert!(title.last() == Some(&0) && text.last() == Some(&0));
        // SAFETY: both strings are NUL-terminated UTF-16 that outlive the call, and a
        // null owner window is allowed.
        unsafe {
            MessageBoxW(
                std::ptr::null_mut(),
                text.as_ptr(),
                title.as_ptr(),
                MB_OK | MB_ICONERROR,
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn app_icon_decodes_embedded_png() {
        let icon = app_icon().expect("embedded icon should decode");
        assert_eq!((icon.width, icon.height), (256, 256));
        assert_eq!(icon.rgba.len(), 256 * 256 * 4);
    }

    #[test]
    fn dialog_text_is_nul_terminated_utf16() {
        const TEXT: [u16; CPU_UNSUPPORTED_MESSAGE.len() + 1] =
            windows_dialog::wide(CPU_UNSUPPORTED_MESSAGE);
        let expected = CPU_UNSUPPORTED_MESSAGE
            .encode_utf16()
            .chain(Some(0))
            .collect::<Vec<_>>();
        assert_eq!(TEXT.as_slice(), expected.as_slice());
        assert_eq!(windows_dialog::wide::<9>("Nomadeer").last(), Some(&0));
    }

    #[test]
    fn runtime_dialog_text_is_nul_terminated_utf16() {
        let message = fatal_error_message("Ünable to create a GL context");
        let wide = windows_dialog::to_wide(&message);
        assert_eq!(wide.last(), Some(&0));
        assert_eq!(
            String::from_utf16(&wide[..wide.len() - 1]).unwrap(),
            message
        );
        assert!(message.contains("Ünable to create a GL context"));
    }

    fn full_x86_64_v3() -> cpu::Registers {
        cpu::Registers {
            leaf1_ecx: cpu::FMA | cpu::MOVBE | cpu::OSXSAVE | cpu::AVX | cpu::F16C,
            leaf7_ebx: cpu::BMI1 | cpu::AVX2 | cpu::BMI2,
            ext1_ecx: cpu::LZCNT,
            xcr0: cpu::XCR0_SSE_AVX | 1,
        }
    }

    #[test]
    fn optimized_build_needs_every_x86_64_v3_feature() {
        assert!(cpu::supports_x86_64_v3(full_x86_64_v3()));
        assert!(!cpu::supports_x86_64_v3(cpu::Registers::default()));

        for bit in [cpu::FMA, cpu::MOVBE, cpu::OSXSAVE, cpu::AVX, cpu::F16C] {
            let mut registers = full_x86_64_v3();
            registers.leaf1_ecx &= !bit;
            assert!(!cpu::supports_x86_64_v3(registers), "leaf 1 bit {bit:#x}");
        }
        for bit in [cpu::BMI1, cpu::AVX2, cpu::BMI2] {
            let mut registers = full_x86_64_v3();
            registers.leaf7_ebx &= !bit;
            assert!(!cpu::supports_x86_64_v3(registers), "leaf 7 bit {bit:#x}");
        }
        let mut registers = full_x86_64_v3();
        registers.ext1_ecx = 0;
        assert!(!cpu::supports_x86_64_v3(registers), "LZCNT");

        // A processor with AVX2 whose OS does not save the AVX registers.
        let mut registers = full_x86_64_v3();
        registers.xcr0 = 0b011;
        assert!(!cpu::supports_x86_64_v3(registers), "XCR0");
    }

    #[cfg(target_arch = "x86_64")]
    #[test]
    fn cpuid_check_agrees_with_runtime_detection() {
        // Test builds are not x86-64-v3 builds, so std detects at run time here.
        use std::arch::is_x86_feature_detected as detected;
        let v3 = detected!("avx")
            && detected!("avx2")
            && detected!("bmi1")
            && detected!("bmi2")
            && detected!("f16c")
            && detected!("fma")
            && detected!("movbe")
            && detected!("lzcnt");
        assert_eq!(cpu::supports_optimized_build(), v3);
    }

    #[test]
    fn app_id_matches_linux_packaging() {
        let desktop = include_str!("../packaging/linux/io.github.TomD3Wiz.Nomadeer.desktop");
        // Compare whole lines so a CRLF checkout on Windows still matches.
        let has_line = |line: String| desktop.lines().any(|candidate| candidate == line);
        assert!(has_line(format!("StartupWMClass={APP_ID}")));
        assert!(has_line(format!("Icon={APP_ID}")));

        let metainfo = include_str!("../packaging/linux/io.github.TomD3Wiz.Nomadeer.metainfo.xml");
        assert!(metainfo.contains(&format!("<id>{APP_ID}</id>")));
        assert!(metainfo.contains(&format!(
            "<launchable type=\"desktop-id\">{APP_ID}.desktop</launchable>"
        )));
    }
}
