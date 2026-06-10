//! Build script: generate the C# P/Invoke bindings for the engine's C ABI (plan §6 / §9.3).
//!
//! csbindgen scans the files holding `#[no_mangle] extern "C"` functions and `#[repr(C)]`
//! types and emits `app/Interop/NativeMethods.g.cs`. The output is **checked in** so the
//! C# build does not require cargo, and so binding changes show up in code review; CI
//! re-runs this and fails on a stale diff.

use std::path::Path;

fn main() {
    // Inputs: the ABI surface (lib.rs) and the POD/callback types (ffi/events.rs).
    println!("cargo:rerun-if-changed=src/lib.rs");
    println!("cargo:rerun-if-changed=src/ffi/events.rs");
    println!("cargo:rerun-if-changed=build.rs");

    let out = Path::new("../app/Interop/NativeMethods.g.cs");
    if let Some(dir) = out.parent() {
        // The app/Interop dir is checked in, but create it defensively for fresh clones.
        let _ = std::fs::create_dir_all(dir);
    }

    let result = csbindgen::Builder::default()
        .input_extern_file("src/lib.rs")
        .input_extern_file("src/ffi/events.rs")
        .csharp_dll_name("optimus_engine")
        .csharp_namespace("Optimus.Interop")
        .csharp_class_name("NativeMethods")
        .csharp_class_accessibility("internal")
        // Entry points are `extern "C"` (cdecl) — match it explicitly on the C# side.
        .csharp_use_function_pointer(true)
        .generate_csharp_file(out);

    if let Err(e) = result {
        // Don't fail the engine build if only the C# emission breaks (e.g. read-only tree on
        // a packaging machine); the checked-in file remains usable. Surface it as a warning.
        println!("cargo:warning=csbindgen failed to generate NativeMethods.g.cs: {e}");
    }
}
