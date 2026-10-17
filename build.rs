//! Gives Windows executables Nomadeer's icon and version details. Other targets
//! need nothing from here.

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed=assets/icons/nomadeer.ico");

    // The build script runs on the host, so check the target rather than cfg!(windows).
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows") {
        return;
    }

    let version = std::env::var("CARGO_PKG_VERSION").expect("cargo sets CARGO_PKG_VERSION");
    let mut resource = winresource::WindowsResource::new();
    resource
        .set_icon("assets/icons/nomadeer.ico")
        .set("ProductName", "Nomadeer")
        .set("FileDescription", "Nomadeer")
        .set("InternalName", "nomadeer")
        .set("OriginalFilename", "nomadeer.exe")
        .set("ProductVersion", &version)
        .set("FileVersion", &version);
    if let Err(error) = resource.compile() {
        panic!("could not embed the Windows icon and version info: {error}");
    }
}
