// GStreamer's installer doesn't touch PATH, so its DLLs are delay-loaded and main() points the loader at them.
fn main() {
    if std::env::var("CARGO_CFG_TARGET_ENV").as_deref() == Ok("msvc") {
        for dll in [
            "glib-2.0-0.dll",
            "gobject-2.0-0.dll",
            "gio-2.0-0.dll",
            "gstreamer-1.0-0.dll",
            "gstapp-1.0-0.dll",
            "gstvideo-1.0-0.dll",
        ] {
            println!("cargo:rustc-link-arg-bins=/DELAYLOAD:{dll}");
        }
        println!("cargo:rustc-link-arg-bins=delayimp.lib");
    }
}
