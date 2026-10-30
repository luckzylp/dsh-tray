//! Embed platform resources that Cargo cannot express directly.
//!
//! Windows executables need a resource section for the blue whale icon shown by
//! Explorer, the taskbar and shortcuts. The resource is compiled only in native
//! Windows builds (the MSVC resource compiler is required); other platforms use
//! the platform bundle/desktop icon instead.

fn main() {
    println!("cargo:rerun-if-changed=assets/app/icon.ico");

    #[cfg(windows)]
    embed_windows_icon();
}

#[cfg(windows)]
fn embed_windows_icon() {
    let mut resource = winresource::WindowsResource::new();
    resource.set_icon("assets/app/icon.ico");
    resource.set("FileDescription", "DSH Tray");
    resource.set("ProductName", "DSH Tray");
    if let Err(error) = resource.compile() {
        panic!("failed to embed the Windows application icon: {error}");
    }
}
