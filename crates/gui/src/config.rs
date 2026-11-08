//! Preferências (JSON na pasta de dados), agendamento, integração com o Explorer e captura de tela.

use atlasdisco_core::pattern::Matcher;
use atlasdisco_core::{snapshot, ScanOptions};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

#[derive(Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    pub threads: usize,
    pub follow_links: bool,
    pub dedup_hardlinks: bool,
    pub streams: bool,
    pub skip_cloud: bool,
    pub include: String,
    pub exclude: String,
    pub regex: bool,
    pub hide_empty: bool,
    pub files_in_tree: bool,
    pub history: bool,
    pub dark: bool,
    pub decimals: usize,
    pub recent: Vec<String>,
    pub cols: Vec<bool>,
    pub export_depth: usize,
    pub export_files: bool,
    pub treemap_depth: usize,
}

impl Default for Config {
    fn default() -> Self {
        Config {
            threads: ScanOptions::default().threads,
            follow_links: false,
            dedup_hardlinks: false,
            streams: false,
            skip_cloud: false,
            include: String::new(),
            exclude: String::new(),
            regex: false,
            hide_empty: false,
            files_in_tree: false,
            history: true,
            dark: true,
            decimals: 1,
            recent: vec![],
            //   Nome  Tam  Aloc  %    Arq  Pas  Mod  Ace   Cri   Atr  Tipo Comp  Méd   Cam   Dono
            cols: vec![
                true, true, true, true, true, true, true, false, false, true, true, false, false,
                false, false, true,
            ],
            export_depth: 3,
            export_files: false,
            treemap_depth: 6,
        }
    }
}

impl Config {
    fn path() -> PathBuf {
        snapshot::data_dir().join("config.json")
    }

    pub fn load() -> Config {
        let mut c: Config = std::fs::read_to_string(Self::path())
            .ok()
            .and_then(|t| serde_json::from_str(&t).ok())
            .unwrap_or_default();
        c.cols.resize(crate::Col::ALL.len(), false);
        c.cols[0] = true;
        c
    }

    pub fn save(&self) {
        let p = Self::path();
        if let Some(d) = p.parent() {
            let _ = std::fs::create_dir_all(d);
        }
        if let Ok(t) = serde_json::to_string_pretty(self) {
            let _ = std::fs::write(p, t);
        }
    }

    pub fn scan_options(&self) -> Result<ScanOptions, String> {
        Ok(ScanOptions {
            threads: self.threads.max(1),
            follow_links: self.follow_links,
            dedup_hardlinks: self.dedup_hardlinks,
            streams: self.streams,
            skip_cloud: self.skip_cloud,
            include: Matcher::new(&self.include, self.regex).map_err(|e| e.to_string())?,
            exclude: Matcher::new(&self.exclude, self.regex).map_err(|e| e.to_string())?,
        })
    }

    pub fn add_recent(&mut self, p: &str) {
        self.recent.retain(|r| !r.eq_ignore_ascii_case(p));
        self.recent.insert(0, p.to_string());
        self.recent.truncate(12);
        self.save();
    }
}

pub struct Sched {
    pub path: String,
    pub time: String,
    pub freq: usize,
    pub out_dir: String,
    pub xlsx: bool,
    pub history: bool,
}

impl Default for Sched {
    fn default() -> Self {
        Sched {
            path: String::new(),
            time: "03:00".into(),
            freq: 0,
            out_dir: String::new(),
            xlsx: true,
            history: true,
        }
    }
}

fn cli_exe() -> Option<PathBuf> {
    let exe = std::env::current_exe().ok()?;
    let name = if cfg!(windows) {
        "atlasdisco-cli.exe"
    } else {
        "atlasdisco-cli"
    };
    let p = exe.with_file_name(name);
    p.exists().then_some(p)
}

/// Cria a tarefa agendada chamando o atlasdisco-cli.
pub fn schedule(s: &Sched) -> String {
    let Some(cli) = cli_exe() else {
        return "atlasdisco-cli não encontrado na pasta do programa.".into();
    };
    let Some((hh, mm)) = s
        .time
        .split_once(':')
        .and_then(|(h, m)| Some((h.trim().parse::<u32>().ok()?, m.trim().parse::<u32>().ok()?)))
    else {
        return "Horário inválido (use HH:MM).".into();
    };
    if hh > 23 || mm > 59 || s.path.trim().is_empty() {
        return "Informe a pasta e um horário válido.".into();
    }
    let mut args = vec!["varrer".to_string(), s.path.trim().to_string()];
    if s.history {
        args.push("--historico".into());
    }
    if s.xlsx && !s.out_dir.trim().is_empty() {
        args.push("-e".into());
        args.push(
            Path::new(s.out_dir.trim())
                .join("AtlasDisco_{data}.xlsx")
                .to_string_lossy()
                .into_owned(),
        );
    }
    let name: String = s
        .path
        .chars()
        .map(|c| if c.is_alphanumeric() { c } else { '_' })
        .collect();
    let freq = [
        atlasdisco_core::ops::Frequency::Daily,
        atlasdisco_core::ops::Frequency::Weekly,
        atlasdisco_core::ops::Frequency::Monthly,
    ][s.freq.min(2)];
    match atlasdisco_core::ops::schedule(name.trim_matches('_'), &cli, &args, freq, hh, mm) {
        Ok(m) => format!("Tarefa criada. {}", m.trim()),
        Err(e) => format!("Erro ao agendar: {}", e.trim()),
    }
}

/// Entrada "Analisar com AtlasDisco" no menu de contexto de pastas e unidades (só usuário atual).
#[cfg(windows)]
pub fn explorer_menu(add: bool) -> String {
    let Ok(exe) = std::env::current_exe() else {
        return "Executável não encontrado".into();
    };
    let mut msgs = Vec::new();
    for (key, arg) in [
        ("Directory", "%1"),
        ("Drive", "%1"),
        ("Directory\\Background", "%V"),
    ] {
        let base = format!("HKCU\\Software\\Classes\\{key}\\shell\\AtlasDisco");
        let ok = if add {
            let a = std::process::Command::new("reg")
                .args(["add", &base, "/ve", "/d", "Analisar com AtlasDisco", "/f"])
                .output();
            let b = std::process::Command::new("reg")
                .args([
                    "add",
                    &base,
                    "/v",
                    "Icon",
                    "/d",
                    &exe.to_string_lossy(),
                    "/f",
                ])
                .output();
            let cmd = format!("\"{}\" \"{arg}\"", exe.display());
            let c = std::process::Command::new("reg")
                .args(["add", &format!("{base}\\command"), "/ve", "/d", &cmd, "/f"])
                .output();
            [a, b, c]
                .iter()
                .all(|r| r.as_ref().is_ok_and(|o| o.status.success()))
        } else {
            std::process::Command::new("reg")
                .args(["delete", &base, "/f"])
                .output()
                .is_ok_and(|o| o.status.success())
        };
        msgs.push(ok);
    }
    match (add, msgs.iter().all(|&x| x)) {
        (true, true) => "Menu de contexto do Explorer adicionado.".into(),
        (false, true) => "Menu de contexto do Explorer removido.".into(),
        _ => "Nem todas as chaves do registro foram alteradas.".into(),
    }
}

#[cfg(windows)]
pub fn restart_as_admin(path: &str) -> bool {
    let Ok(exe) = std::env::current_exe() else {
        return false;
    };
    let q = |s: &str| s.replace('\'', "''");
    let args = if path.trim().is_empty() {
        String::new()
    } else {
        format!(" -ArgumentList '\"{}\"'", q(path.trim()))
    };
    let cmd = format!(
        "Start-Process -FilePath '{}'{args} -Verb RunAs",
        q(&exe.to_string_lossy())
    );
    std::process::Command::new("powershell")
        .args(["-NoProfile", "-WindowStyle", "Hidden", "-Command", &cmd])
        .spawn()
        .is_ok()
}

pub fn save_png(path: &Path, img: &eframe::egui::ColorImage) {
    let Ok(f) = std::fs::File::create(path) else {
        return;
    };
    let mut enc = png::Encoder::new(
        std::io::BufWriter::new(f),
        img.size[0] as u32,
        img.size[1] as u32,
    );
    enc.set_color(png::ColorType::Rgba);
    enc.set_depth(png::BitDepth::Eight);
    if let Ok(mut w) = enc.write_header() {
        let _ = w.write_image_data(img.as_raw());
    }
}
