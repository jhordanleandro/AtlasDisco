//! Biblioteca nativa carregada pela NativeActivity do APK (libatlasdisco.so).

#[cfg(target_os = "android")]
#[no_mangle]
fn android_main(app: winit::platform::android::activity::AndroidApp) {
    atlasdisco_gui::android::init(app.clone());
    let opts = eframe::NativeOptions {
        android_app: Some(app),
        ..Default::default()
    };
    if let Err(e) = atlasdisco_gui::run(opts, None, None) {
        atlasdisco_gui::android::log(&format!("erro ao iniciar: {e}"));
    }
}
