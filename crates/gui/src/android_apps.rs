//! Espaço ocupado por cada app no Android (StorageStatsManager), que exige o "Acesso ao uso".

use crate::android::{log, sdk_int, with_env};
use atlasdisco_core::scan::AppStorage;
use jni::objects::{JObject, JString, JValue};
use jni::JNIEnv;
use std::sync::Mutex;
use std::time::{Duration, Instant};

static USAGE: Mutex<Option<(Instant, bool)>> = Mutex::new(None);

fn package_name(env: &mut JNIEnv, act: &JObject) -> jni::errors::Result<String> {
    let pkg: JString = env
        .call_method(act, "getPackageName", "()Ljava/lang/String;", &[])?
        .l()?
        .into();
    let name: String = env.get_string(&pkg)?.into();
    Ok(name)
}

fn check_usage() -> bool {
    with_env(|env, act| {
        let svc = env.new_string("appops")?;
        let ops = env
            .call_method(
                act,
                "getSystemService",
                "(Ljava/lang/String;)Ljava/lang/Object;",
                &[JValue::Object(&svc)],
            )?
            .l()?;
        let uid = env
            .call_static_method("android/os/Process", "myUid", "()I", &[])?
            .i()?;
        let pkg = package_name(env, act)?;
        let op = env.new_string("android:get_usage_stats")?;
        let pkg = env.new_string(pkg)?;
        let name = if sdk_int(env)? >= 29 {
            "unsafeCheckOpNoThrow"
        } else {
            "checkOpNoThrow"
        };
        let mode = env
            .call_method(
                &ops,
                name,
                "(Ljava/lang/String;ILjava/lang/String;)I",
                &[JValue::Object(&op), JValue::Int(uid), JValue::Object(&pkg)],
            )?
            .i()?;
        if mode == 3 {
            // MODE_DEFAULT: vale a permissão declarada no manifesto.
            let p = env.new_string("android.permission.PACKAGE_USAGE_STATS")?;
            return Ok(env
                .call_method(
                    act,
                    "checkSelfPermission",
                    "(Ljava/lang/String;)I",
                    &[JValue::Object(&p)],
                )?
                .i()?
                == 0);
        }
        Ok(mode == 0)
    })
    .unwrap_or(false)
}

/// "Acesso ao uso" concedido? (cache de 2 s)
pub fn has_usage_access() -> bool {
    let mut g = USAGE.lock().unwrap();
    match *g {
        Some((t, v)) if t.elapsed() < Duration::from_secs(2) => v,
        _ => {
            let v = check_usage();
            *g = Some((Instant::now(), v));
            v
        }
    }
}

/// Abre a tela "Acesso ao uso" do sistema (direto no app quando o aparelho aceita).
pub fn request_usage_access() {
    with_env(|env, act| {
        let pkg = package_name(env, act)?;
        let action = env.new_string("android.settings.USAGE_ACCESS_SETTINGS")?;
        let uri_s = env.new_string(format!("package:{pkg}"))?;
        let uri = env
            .call_static_method(
                "android/net/Uri",
                "parse",
                "(Ljava/lang/String;)Landroid/net/Uri;",
                &[JValue::Object(&uri_s)],
            )?
            .l()?;
        let intent = env.new_object(
            "android/content/Intent",
            "(Ljava/lang/String;Landroid/net/Uri;)V",
            &[JValue::Object(&action), JValue::Object(&uri)],
        )?;
        if env
            .call_method(
                act,
                "startActivity",
                "(Landroid/content/Intent;)V",
                &[JValue::Object(&intent)],
            )
            .is_err()
        {
            // Alguns aparelhos não aceitam o pacote na URI: abre a lista geral.
            let _ = env.exception_clear();
            let plain = env.new_object(
                "android/content/Intent",
                "(Ljava/lang/String;)V",
                &[JValue::Object(&action)],
            )?;
            env.call_method(
                act,
                "startActivity",
                "(Landroid/content/Intent;)V",
                &[JValue::Object(&plain)],
            )?;
        }
        Ok(())
    });
    *USAGE.lock().unwrap() = None;
}

/// Espaço de cada app instalado (APK, dados, cache), pela API oficial do Android.
pub fn app_storage() -> Vec<AppStorage> {
    with_env(|env, act| {
        let svc = env.new_string("storagestats")?;
        let ssm = env
            .call_method(act, "getSystemService", "(Ljava/lang/String;)Ljava/lang/Object;", &[JValue::Object(&svc)])?
            .l()?;
        let pm = env.call_method(act, "getPackageManager", "()Landroid/content/pm/PackageManager;", &[])?.l()?;
        let list = env.call_method(&pm, "getInstalledApplications", "(I)Ljava/util/List;", &[JValue::Int(0)])?.l()?;
        let uuid = env
            .get_static_field("android/os/storage/StorageManager", "UUID_DEFAULT", "Ljava/util/UUID;")?
            .l()?;
        let user = env
            .call_static_method("android/os/Process", "myUserHandle", "()Landroid/os/UserHandle;", &[])?
            .l()?;
        let n = env.call_method(&list, "size", "()I", &[])?.i()?;
        let mut out = Vec::new();
        for i in 0..n {
            // Um quadro local por app, para não estourar a tabela de referências da JNI.
            let r = env.with_local_frame(16, |env| -> jni::errors::Result<Option<AppStorage>> {
                let ai = env.call_method(&list, "get", "(I)Ljava/lang/Object;", &[JValue::Int(i)])?.l()?;
                let pkg: JString = env.get_field(&ai, "packageName", "Ljava/lang/String;")?.l()?.into();
                let package: String = env.get_string(&pkg)?.into();
                let label = env
                    .call_method(
                        &pm,
                        "getApplicationLabel",
                        "(Landroid/content/pm/ApplicationInfo;)Ljava/lang/CharSequence;",
                        &[JValue::Object(&ai)],
                    )?
                    .l()?;
                let label: JString = env.call_method(&label, "toString", "()Ljava/lang/String;", &[])?.l()?.into();
                let label: String = env.get_string(&label)?.into();
                let st = match env.call_method(
                    &ssm,
                    "queryStatsForPackage",
                    "(Ljava/util/UUID;Ljava/lang/String;Landroid/os/UserHandle;)Landroid/app/usage/StorageStats;",
                    &[JValue::Object(&uuid), JValue::Object(&pkg), JValue::Object(&user)],
                ) {
                    Ok(v) => v.l()?,
                    Err(_) => {
                        let _ = env.exception_clear();
                        return Ok(None);
                    }
                };
                let mut get = |m: &str| -> jni::errors::Result<u64> { Ok(env.call_method(&st, m, "()J", &[])?.j()?.max(0) as u64) };
                let (app, data, cache) = (get("getAppBytes")?, get("getDataBytes")?, get("getCacheBytes")?);
                Ok(Some(AppStorage { label, package, app, data, cache }))
            });
            match r {
                Ok(Some(a)) => out.push(a),
                Ok(None) => {}
                Err(e) => {
                    let _ = env.exception_clear();
                    log(&format!("app {i}: {e}"));
                }
            }
        }
        log(&format!("espaço lido de {} apps", out.len()));
        Ok(out)
    })
    .unwrap_or_default()
}
