//! Embeds the app icon (and version details) in the Windows `.exe`, for Explorer, the
//! taskbar and the installer's Start menu shortcut. Other targets build nothing here.

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed=assets/icon/icon.ico");
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        let mut res = winresource::WindowsResource::new();
        res.set_icon("assets/icon/icon.ico");
        res.compile()
            .expect("couldn't embed the icon in the Windows executable");
    }
}
