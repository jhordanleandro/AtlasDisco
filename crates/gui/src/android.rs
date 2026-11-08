//! Integração com o Android: permissão de acesso aos arquivos, teclado virtual e botão "voltar".

use crate::{Act, App};
use eframe::egui::{self, Color32, RichText};
use jni::objects::{JObject, JString, JValue};
use jni::{JNIEnv, JavaVM};
use std::sync::atomic::{AtomicBool, Ordering::Relaxed};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};
use winit::platform::android::activity::AndroidApp;

static ANDROID: OnceLock<AndroidApp> = OnceLock::new();
static KEYBOARD: AtomicBool = AtomicBool::new(false);
static ACCESS: Mutex<Option<(Instant, bool)>> = Mutex::new(None);

/// Chamado pelo `android_main` antes de abrir a janela.
pub fn init(app: AndroidApp) {
    if let Some(p) = app.internal_data_path() {
        // Pasta privada do app para configurações e histórico (lida por snapshot::data_dir).
        std::env::set_var("ATLASDISCO_DATA", p);
    }
    let _ = ANDROID.set(app);
    std::panic::set_hook(Box::new(|info| log(&format!("pânico: {info}"))));
}

/// Escreve no logcat (tag "AtlasDisco").
pub fn log(msg: &str) {
    extern "C" {
        fn __android_log_write(
            prio: i32,
            tag: *const std::ffi::c_char,
            text: *const std::ffi::c_char,
        ) -> i32;
    }
    let tag = std::ffi::CString::new("AtlasDisco").unwrap();
    let text = std::ffi::CString::new(msg.replace('\0', " ")).unwrap_or_default();
    unsafe {
        __android_log_write(6, tag.as_ptr(), text.as_ptr());
    }
}

pub(crate) fn with_env<R>(
    f: impl FnOnce(&mut JNIEnv, &JObject) -> jni::errors::Result<R>,
) -> Option<R> {
    let app = ANDROID.get()?;
    let vm = unsafe { JavaVM::from_raw(app.vm_as_ptr() as *mut jni::sys::JavaVM) }.ok()?;
    let mut env = vm.attach_current_thread().ok()?;
    // Referência global mantida pela NativeActivity: não deve ser liberada aqui.
    let activity = unsafe { JObject::from_raw(app.activity_as_ptr() as jni::sys::jobject) };
    let r = f(&mut env, &activity);
    if env.exception_check().unwrap_or(false) {
        let _ = env.exception_describe();
        let _ = env.exception_clear();
    }
    match r {
        Ok(v) => Some(v),
        Err(e) => {
            log(&format!("JNI: {e}"));
            None
        }
    }
}

pub(crate) fn sdk_int(env: &mut JNIEnv) -> jni::errors::Result<i32> {
    env.get_static_field("android/os/Build$VERSION", "SDK_INT", "I")?
        .i()
}

/// O app consegue ler todo o armazenamento? (Android 11+: "acesso a todos os arquivos".)
fn check_access() -> bool {
    with_env(|env, act| {
        if sdk_int(env)? >= 30 {
            env.call_static_method(
                "android/os/Environment",
                "isExternalStorageManager",
                "()Z",
                &[],
            )?
            .z()
        } else {
            let p = env.new_string("android.permission.READ_EXTERNAL_STORAGE")?;
            let r = env
                .call_method(
                    act,
                    "checkSelfPermission",
                    "(Ljava/lang/String;)I",
                    &[JValue::Object(&p)],
                )?
                .i()?;
            Ok(r == 0)
        }
    })
    .unwrap_or(false)
}

/// Resultado em cache por 2 s (a verificação passa pela JVM).
pub fn has_access() -> bool {
    let mut g = ACCESS.lock().unwrap();
    match *g {
        Some((t, v)) if t.elapsed() < Duration::from_secs(2) => v,
        _ => {
            let v = check_access();
            *g = Some((Instant::now(), v));
            v
        }
    }
}

/// Abre a tela do sistema para conceder o acesso (ou o diálogo de permissão em Android 10 e anteriores).
pub fn request_access() {
    with_env(|env, act| {
        if sdk_int(env)? >= 30 {
            let pkg: JString = env
                .call_method(act, "getPackageName", "()Ljava/lang/String;", &[])?
                .l()?
                .into();
            let pkg: String = env.get_string(&pkg)?.into();
            let uri_s = env.new_string(format!("package:{pkg}"))?;
            let uri = env
                .call_static_method(
                    "android/net/Uri",
                    "parse",
                    "(Ljava/lang/String;)Landroid/net/Uri;",
                    &[JValue::Object(&uri_s)],
                )?
                .l()?;
            let action =
                env.new_string("android.settings.MANAGE_APP_ALL_FILES_ACCESS_PERMISSION")?;
            let intent = env.new_object(
                "android/content/Intent",
                "(Ljava/lang/String;Landroid/net/Uri;)V",
                &[JValue::Object(&action), JValue::Object(&uri)],
            )?;
            env.call_method(
                act,
                "startActivity",
                "(Landroid/content/Intent;)V",
                &[JValue::Object(&intent)],
            )?;
        } else {
            let cls = env.find_class("java/lang/String")?;
            let perms = env.new_object_array(2, cls, JObject::null())?;
            let r = env.new_string("android.permission.READ_EXTERNAL_STORAGE")?;
            let w = env.new_string("android.permission.WRITE_EXTERNAL_STORAGE")?;
            env.set_object_array_element(&perms, 0, r)?;
            env.set_object_array_element(&perms, 1, w)?;
            env.call_method(
                act,
                "requestPermissions",
                "([Ljava/lang/String;I)V",
                &[JValue::Object(&perms), JValue::Int(1)],
            )?;
        }
        Ok(())
    });
    *ACCESS.lock().unwrap() = None;
}

static INSETS: Mutex<Option<(Instant, (i32, i32))>> = Mutex::new(None);

/// Altura (px) das barras do sistema + teclado: (topo, base).
fn system_insets() -> (i32, i32) {
    with_env(|env, act| {
        let win = env
            .call_method(act, "getWindow", "()Landroid/view/Window;", &[])?
            .l()?;
        let decor = env
            .call_method(&win, "getDecorView", "()Landroid/view/View;", &[])?
            .l()?;
        let ins = env
            .call_method(
                &decor,
                "getRootWindowInsets",
                "()Landroid/view/WindowInsets;",
                &[],
            )?
            .l()?;
        if ins.is_null() {
            return Ok((0, 0));
        }
        if sdk_int(env)? >= 30 {
            let bars = env
                .call_static_method("android/view/WindowInsets$Type", "systemBars", "()I", &[])?
                .i()?;
            let ime = env
                .call_static_method("android/view/WindowInsets$Type", "ime", "()I", &[])?
                .i()?;
            let cut = env
                .call_static_method(
                    "android/view/WindowInsets$Type",
                    "displayCutout",
                    "()I",
                    &[],
                )?
                .i()?;
            let i = env
                .call_method(
                    &ins,
                    "getInsets",
                    "(I)Landroid/graphics/Insets;",
                    &[JValue::Int(bars | ime | cut)],
                )?
                .l()?;
            Ok((
                env.get_field(&i, "top", "I")?.i()?,
                env.get_field(&i, "bottom", "I")?.i()?,
            ))
        } else {
            let t = env
                .call_method(&ins, "getSystemWindowInsetTop", "()I", &[])?
                .i()?;
            let b = env
                .call_method(&ins, "getSystemWindowInsetBottom", "()I", &[])?
                .i()?;
            Ok((t, b))
        }
    })
    .unwrap_or((0, 0))
}

/// Faixas vazias no topo e na base com a altura das barras do sistema (chamar antes dos outros painéis).
pub fn insets(ctx: &egui::Context) {
    let (top, bottom) = {
        let mut g = INSETS.lock().unwrap();
        match *g {
            Some((t, v)) if t.elapsed() < Duration::from_millis(400) => v,
            _ => {
                let v = system_insets();
                *g = Some((Instant::now(), v));
                v
            }
        }
    };
    let ppp = ctx.pixels_per_point();
    let fill = ctx.style().visuals.panel_fill;
    if top > 0 {
        egui::TopBottomPanel::top("android-topo")
            .exact_height(top as f32 / ppp)
            .frame(egui::Frame::NONE.fill(fill))
            .show_separator_line(false)
            .show(ctx, |_| {});
    }
    if bottom > 0 {
        egui::TopBottomPanel::bottom("android-base")
            .exact_height(bottom as f32 / ppp)
            .frame(egui::Frame::NONE.fill(fill))
            .show_separator_line(false)
            .show(ctx, |_| {});
    }
    // As barras mudam com o teclado e a rotação: continua checando.
    ctx.request_repaint_after(Duration::from_millis(500));
}

/// Executado a cada quadro, antes da área central.
pub fn frame(ctx: &egui::Context, app: &mut App, _acts: &mut Vec<Act>) {
    // Teclado virtual acompanha o foco dos campos de texto.
    let wants = ctx.wants_keyboard_input();
    if wants != KEYBOARD.load(Relaxed) {
        if let Some(a) = ANDROID.get() {
            if wants {
                a.show_soft_input(true);
            } else {
                a.hide_soft_input(true);
            }
        }
        KEYBOARD.store(wants, Relaxed);
    }
    if !has_access() {
        egui::TopBottomPanel::top("permissao").show(ctx, |ui| {
            ui.add_space(6.0);
            ui.label(RichText::new("Para medir todas as pastas, o AtlasDisco precisa de acesso a todos os arquivos.").strong());
            ui.label(RichText::new("Sem isso, o Android só mostra fotos, vídeos e músicas. O app só lê; nada sai do aparelho.").weak());
            if ui.button(RichText::new("Conceder acesso").color(Color32::WHITE)).clicked() {
                request_access();
            }
            ui.add_space(6.0);
        });
        return;
    }
    // Detalha "[Apps, dados de apps e sistema]" por app assim que o "Acesso ao uso" existir.
    let pending = app.scan.as_ref().is_some_and(|s| {
        s.root
            .children
            .iter()
            .any(|c| c.synthetic && c.children.is_empty())
    });
    if pending && app.apps_gen != app.gen {
        if crate::android_apps::has_usage_access() {
            if app.bg.is_none() {
                app.load_app_sizes();
            }
        } else {
            egui::TopBottomPanel::top("uso").show(ctx, |ui| {
                ui.add_space(6.0);
                ui.label(RichText::new("Quer ver quanto cada app ocupa (app, dados e cache)?").strong());
                ui.label(RichText::new("O Android só informa isso com o \"Acesso ao uso\". O AtlasDisco lê apenas os tamanhos.").weak());
                ui.horizontal_wrapped(|ui| {
                    if ui.button("Conceder Acesso ao uso").clicked() {
                        crate::android_apps::request_usage_access();
                    }
                    if ui.button("Agora não").clicked() {
                        app.apps_gen = app.gen;
                    }
                });
                ui.add_space(6.0);
            });
        }
    }
}
