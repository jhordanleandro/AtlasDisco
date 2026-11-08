//! Operações em massa: lixeira, excluir, mover, copiar, renomear, deduplicar com hardlink, compressão NTFS e agendamento.

use regex::Regex;
use std::fs;
use std::path::{Path, PathBuf};

#[derive(Debug, Default)]
pub struct Report {
    pub ok: usize,
    pub failed: Vec<(PathBuf, String)>,
    pub freed: u64,
}

impl Report {
    fn fail(&mut self, p: &Path, e: impl ToString) {
        self.failed.push((p.to_path_buf(), e.to_string()));
    }
}

#[cfg(target_os = "android")]
pub fn to_trash(paths: &[PathBuf]) -> Report {
    let mut r = Report::default();
    for p in paths {
        r.fail(p, "o Android não tem lixeira para arquivos; use Excluir");
    }
    r
}

#[cfg(not(target_os = "android"))]
pub fn to_trash(paths: &[PathBuf]) -> Report {
    let mut r = Report::default();
    for p in paths {
        let size = size_of(p);
        match trash::delete(p) {
            Ok(()) => {
                r.ok += 1;
                r.freed += size;
            }
            Err(e) => r.fail(p, e),
        }
    }
    r
}

pub fn delete_permanently(paths: &[PathBuf]) -> Report {
    let mut r = Report::default();
    for p in paths {
        let size = size_of(p);
        let res = if p.is_dir() {
            fs::remove_dir_all(p)
        } else {
            fs::remove_file(p)
        };
        match res {
            Ok(()) => {
                r.ok += 1;
                r.freed += size;
            }
            Err(e) => r.fail(p, e),
        }
    }
    r
}

fn size_of(p: &Path) -> u64 {
    fs::symlink_metadata(p)
        .map(|m| if m.is_file() { m.len() } else { 0 })
        .unwrap_or(0)
}

/// Copia ou move para `dest`. Com `keep_tree`, recria a estrutura relativa a `base` (útil para arquivar).
pub fn transfer(paths: &[PathBuf], dest: &Path, base: Option<&Path>, mov: bool) -> Report {
    let mut r = Report::default();
    for p in paths {
        let rel = base
            .and_then(|b| p.strip_prefix(b).ok())
            .map(Path::to_path_buf)
            .unwrap_or_else(|| PathBuf::from(p.file_name().unwrap_or_default()));
        let target = unique(&dest.join(rel));
        if let Some(parent) = target.parent() {
            if let Err(e) = fs::create_dir_all(parent) {
                r.fail(p, e);
                continue;
            }
        }
        let res = if mov {
            fs::rename(p, &target).or_else(|_| {
                copy_any(p, &target).and_then(|_| {
                    if p.is_dir() {
                        fs::remove_dir_all(p)
                    } else {
                        fs::remove_file(p)
                    }
                })
            })
        } else {
            copy_any(p, &target)
        };
        match res {
            Ok(()) => r.ok += 1,
            Err(e) => r.fail(p, e),
        }
    }
    r
}

fn copy_any(from: &Path, to: &Path) -> std::io::Result<()> {
    if from.is_dir() {
        fs::create_dir_all(to)?;
        for e in fs::read_dir(from)? {
            let e = e?;
            copy_any(&e.path(), &to.join(e.file_name()))?;
        }
        Ok(())
    } else {
        fs::copy(from, to).map(|_| ())
    }
}

/// Nome livre: "a.txt" → "a (2).txt" se já existir.
pub fn unique(p: &Path) -> PathBuf {
    if !p.exists() {
        return p.to_path_buf();
    }
    let stem = p
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_default();
    let ext = p
        .extension()
        .map(|e| format!(".{}", e.to_string_lossy()))
        .unwrap_or_default();
    (2..)
        .map(|i| p.with_file_name(format!("{stem} ({i}){ext}")))
        .find(|c| !c.exists())
        .unwrap()
}

/// Renomeia trocando `find` (regex) por `replace` no nome. `dry_run` só calcula os novos nomes.
pub fn rename(
    paths: &[PathBuf],
    find: &str,
    replace: &str,
    dry_run: bool,
) -> Result<(Vec<(PathBuf, PathBuf)>, Report), String> {
    let rx = Regex::new(find).map_err(|e| e.to_string())?;
    let mut plan = Vec::new();
    let mut r = Report::default();
    for p in paths {
        let name = p
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        let new = rx.replace_all(&name, replace).into_owned();
        if new == name || new.is_empty() {
            continue;
        }
        let target = p.with_file_name(&new);
        if !dry_run {
            if target.exists() {
                r.fail(p, "já existe um item com o novo nome");
                continue;
            }
            match fs::rename(p, &target) {
                Ok(()) => r.ok += 1,
                Err(e) => {
                    r.fail(p, e);
                    continue;
                }
            }
        }
        plan.push((p.clone(), target));
    }
    Ok((plan, r))
}

/// Troca cada cópia pelo hardlink do primeiro arquivo do grupo (mesmo volume). O original renomeado só é
/// apagado depois de o link existir; em caso de erro, volta ao nome original.
pub fn hardlink_group(keep: &Path, copies: &[PathBuf]) -> Report {
    let mut r = Report::default();
    for c in copies {
        let tmp = c.with_file_name(format!(
            "{}.atlasdisco-tmp",
            c.file_name().unwrap_or_default().to_string_lossy()
        ));
        let size = size_of(c);
        if let Err(e) = fs::rename(c, &tmp) {
            r.fail(c, e);
            continue;
        }
        match fs::hard_link(keep, c) {
            Ok(()) => {
                let _ = fs::remove_file(&tmp);
                r.ok += 1;
                r.freed += size;
            }
            Err(e) => {
                let _ = fs::rename(&tmp, c);
                r.fail(c, e);
            }
        }
    }
    r
}

/// Compressão NTFS (Windows: compact.exe). Em outros sistemas retorna erro.
pub fn ntfs_compress(path: &Path, compress: bool) -> Result<String, String> {
    if !cfg!(windows) {
        return Err("compressão NTFS só existe no Windows".into());
    }
    let mut cmd = std::process::Command::new("compact");
    cmd.arg(if compress { "/c" } else { "/u" });
    if path.is_dir() {
        // /s:<pasta> aplica à pasta e a todo o conteúdo (e marca a pasta para novos arquivos).
        let mut s = std::ffi::OsString::from("/s:");
        s.push(path.as_os_str());
        cmd.arg(s).args(["/i", "/q"]);
    } else {
        cmd.args(["/i", "/q"]).arg(path);
    }
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
    }
    let out = cmd.output().map_err(|e| e.to_string())?;
    let text = String::from_utf8_lossy(&out.stdout).into_owned();
    if out.status.success() {
        Ok(text)
    } else {
        Err(text + &String::from_utf8_lossy(&out.stderr))
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Frequency {
    Daily,
    Weekly,
    Monthly,
}

/// Cria uma varredura agendada que roda `exe args`. Windows: Agendador de Tarefas; outros: linha para o crontab.
pub fn schedule(
    name: &str,
    exe: &Path,
    args: &[String],
    freq: Frequency,
    hh: u32,
    mm: u32,
) -> Result<String, String> {
    let quoted: Vec<String> = std::iter::once(exe.to_string_lossy().into_owned())
        .chain(args.iter().cloned())
        .map(|a| quote(&a))
        .collect();
    let line = quoted.join(" ");
    if cfg!(windows) {
        let sc = match freq {
            Frequency::Daily => "DAILY",
            Frequency::Weekly => "WEEKLY",
            Frequency::Monthly => "MONTHLY",
        };
        let out = std::process::Command::new("schtasks")
            .args([
                "/Create",
                "/F",
                "/SC",
                sc,
                "/ST",
                &format!("{hh:02}:{mm:02}"),
                "/TN",
                &format!("AtlasDisco\\{name}"),
                "/TR",
                &line,
            ])
            .output()
            .map_err(|e| e.to_string())?;
        let msg = String::from_utf8_lossy(&out.stdout).into_owned()
            + &String::from_utf8_lossy(&out.stderr);
        if out.status.success() {
            Ok(msg)
        } else {
            Err(msg)
        }
    } else {
        let when = match freq {
            Frequency::Daily => format!("{mm} {hh} * * *"),
            Frequency::Weekly => format!("{mm} {hh} * * 1"),
            Frequency::Monthly => format!("{mm} {hh} 1 * *"),
        };
        Ok(format!("Adicione ao crontab (crontab -e):\n{when} {line}"))
    }
}

pub fn unschedule(name: &str) -> Result<String, String> {
    let out = std::process::Command::new("schtasks")
        .args(["/Delete", "/F", "/TN", &format!("AtlasDisco\\{name}")])
        .output()
        .map_err(|e| e.to_string())?;
    let msg =
        String::from_utf8_lossy(&out.stdout).into_owned() + &String::from_utf8_lossy(&out.stderr);
    if out.status.success() {
        Ok(msg)
    } else {
        Err(msg)
    }
}

fn quote(a: &str) -> String {
    if a.is_empty() || a.contains([' ', '\t', '"']) {
        format!("\"{}\"", a.replace('"', "\\\""))
    } else {
        a.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn renomear_hardlink_mover() {
        let t = tempfile::tempdir().unwrap();
        let a = t.path().join("foto 01.jpg");
        let b = t.path().join("foto 02.jpg");
        fs::write(&a, b"igual").unwrap();
        fs::write(&b, b"igual").unwrap();
        let r = hardlink_group(&a, std::slice::from_ref(&b));
        assert_eq!(r.ok, 1, "{:?}", r.failed);
        assert_eq!(fs::read(&b).unwrap(), b"igual");
        let (plan, rep) = rename(&[a.clone(), b.clone()], " ", "_", false).unwrap();
        assert_eq!(plan.len(), 2);
        assert_eq!(rep.ok, 2);
        assert!(t.path().join("foto_01.jpg").exists());
        let dest = t.path().join("arquivo");
        let r = transfer(&[t.path().join("foto_01.jpg")], &dest, Some(t.path()), true);
        assert_eq!(r.ok, 1);
        assert!(dest.join("foto_01.jpg").exists());
        assert_eq!(
            unique(&dest.join("foto_01.jpg")),
            dest.join("foto_01 (2).jpg")
        );
    }
}
