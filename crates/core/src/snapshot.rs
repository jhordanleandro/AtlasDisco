//! Instantâneos (.atlas = JSON compactado com gzip), comparação entre varreduras e histórico por pasta.

use crate::model::{eq_name, Node, Scan};
use flate2::{read::GzDecoder, write::GzEncoder, Compression};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs::{self, File};
use std::io::{BufReader, BufWriter, Write};
use std::path::{Path, PathBuf};

pub const EXT: &str = "atlas";

pub fn save(scan: &Scan, path: &Path) -> std::io::Result<()> {
    let tmp = path.with_extension("atlas.tmp");
    {
        let f = BufWriter::new(File::create(&tmp)?);
        let mut gz = GzEncoder::new(f, Compression::fast());
        serde_json::to_writer(&mut gz, scan).map_err(std::io::Error::other)?;
        gz.finish()?.flush()?;
    }
    fs::rename(&tmp, path)
}

pub fn load(path: &Path) -> std::io::Result<Scan> {
    let f = BufReader::new(File::open(path)?);
    let s: Scan = serde_json::from_reader(GzDecoder::new(f))
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
    if s.format > Scan::FORMAT {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "instantâneo de versão mais nova",
        ));
    }
    Ok(s)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Status {
    Added,
    Removed,
    Changed,
}

#[derive(Clone, Debug)]
pub struct Diff {
    /// Caminho relativo à raiz comparada.
    pub path: PathBuf,
    pub is_dir: bool,
    pub level: usize,
    pub old_size: u64,
    pub new_size: u64,
    pub old_files: u64,
    pub new_files: u64,
    pub status: Status,
}

impl Diff {
    pub fn delta(&self) -> i64 {
        self.new_size as i64 - self.old_size as i64
    }
}

impl Status {
    pub fn label(self) -> &'static str {
        match self {
            Status::Added => "novo",
            Status::Removed => "removido",
            Status::Changed => "alterado",
        }
    }
}

pub fn diff_table(title: String, diffs: &[Diff]) -> crate::export::Table {
    use crate::export::Cell;
    crate::export::Table {
        title,
        headers: [
            "Caminho",
            "Situação",
            "Antes",
            "Depois",
            "Diferença (bytes)",
            "Arquivos antes",
            "Arquivos depois",
        ]
        .map(String::from)
        .to_vec(),
        rows: diffs
            .iter()
            .map(|d| {
                vec![
                    Cell::Text(d.path.to_string_lossy().into_owned()),
                    Cell::Text(d.status.label().into()),
                    Cell::Bytes(d.old_size),
                    Cell::Bytes(d.new_size),
                    Cell::Int(d.delta()),
                    Cell::Int(d.old_files as i64),
                    Cell::Int(d.new_files as i64),
                ]
            })
            .collect(),
        indent: vec![],
    }
}

/// Diferenças entre duas árvores até `max_depth` níveis. Itens iguais não aparecem.
pub fn compare(old: &Node, new: &Node, max_depth: usize, files: bool) -> Vec<Diff> {
    let mut out = Vec::new();
    rec(
        Some(old),
        Some(new),
        PathBuf::new(),
        0,
        max_depth,
        files,
        &mut out,
    );
    out.sort_by(|a, b| {
        b.delta()
            .unsigned_abs()
            .cmp(&a.delta().unsigned_abs())
            .then(a.path.cmp(&b.path))
    });
    out
}

fn rec(
    old: Option<&Node>,
    new: Option<&Node>,
    rel: PathBuf,
    lvl: usize,
    max: usize,
    files: bool,
    out: &mut Vec<Diff>,
) {
    let any = new.or(old).unwrap();
    if !any.is_dir && !files {
        return;
    }
    let (os, ns) = (old.map_or(0, |n| n.size), new.map_or(0, |n| n.size));
    let (of, nf) = (
        old.map_or(0, |n| if n.is_dir { n.files } else { 1 }),
        new.map_or(0, |n| if n.is_dir { n.files } else { 1 }),
    );
    let status = match (old, new) {
        (None, Some(_)) => Status::Added,
        (Some(_), None) => Status::Removed,
        _ => Status::Changed,
    };
    if lvl > 0 && (status != Status::Changed || os != ns || of != nf) {
        out.push(Diff {
            path: rel.clone(),
            is_dir: any.is_dir,
            level: lvl,
            old_size: os,
            new_size: ns,
            old_files: of,
            new_files: nf,
            status,
        });
    }
    if lvl >= max || !any.is_dir {
        return;
    }
    // Só desce se algo mudou dentro (ou se é novo/removido).
    if status == Status::Changed
        && os == ns
        && of == nf
        && old.map(|n| n.mtime) == new.map(|n| n.mtime)
    {
        return;
    }
    let key = |s: &str| {
        if cfg!(target_os = "linux") {
            s.to_string()
        } else {
            s.to_lowercase()
        }
    };
    let mut olds: HashMap<String, &Node> = HashMap::new();
    if let Some(o) = old {
        for c in &o.children {
            olds.insert(key(&c.name), c);
        }
    }
    if let Some(n) = new {
        for c in &n.children {
            let o = olds.remove(&key(&c.name));
            rec(o, Some(c), rel.join(&c.name), lvl + 1, max, files, out);
        }
    }
    for (_, o) in olds {
        rec(Some(o), None, rel.join(&o.name), lvl + 1, max, files, out);
    }
}

/// Encontra na árvore `scan` o nó correspondente a `path` (absoluto).
pub fn locate<'a>(scan: &'a Scan, path: &Path) -> Option<&'a Node> {
    // Compara os componentes da raiz sem diferenciar maiúsculas (Windows/macOS).
    let root: Vec<_> = scan.root_path.components().collect();
    let all: Vec<_> = path.components().collect();
    if all.len() < root.len()
        || !root.iter().zip(&all).all(|(a, b)| {
            eq_name(
                &a.as_os_str().to_string_lossy(),
                &b.as_os_str().to_string_lossy(),
            )
        })
    {
        return None;
    }
    let mut n = &scan.root;
    for part in &all[root.len()..] {
        let name = part.as_os_str().to_string_lossy();
        n = n.children.iter().find(|c| eq_name(&c.name, &name))?;
    }
    Some(n)
}

// ---------------------------------------------------------------- histórico

/// Pasta de dados. Modo portátil: `portatil.txt` ao lado do executável → `dados\` ali mesmo.
pub fn data_dir() -> PathBuf {
    // Definida pelo app Android (pasta privada) ou por quem quiser escolher outro lugar.
    if let Some(d) = std::env::var_os("ATLASDISCO_DATA") {
        return PathBuf::from(d);
    }
    if let Ok(exe) = std::env::current_exe() {
        if let Some(d) = exe.parent() {
            if d.join("portatil.txt").exists() {
                return d.join("dados");
            }
        }
    }
    dirs::data_local_dir()
        .unwrap_or_else(std::env::temp_dir)
        .join("AtlasDisco")
}

fn history_dir(root: &Path) -> PathBuf {
    let key = root.to_string_lossy().to_lowercase();
    // FNV-1a: nome de pasta estável por caminho
    let mut h: u64 = 0xcbf29ce484222325;
    for b in key.bytes() {
        h ^= b as u64;
        h = h.wrapping_mul(0x100000001b3);
    }
    data_dir().join("historico").join(format!("{h:016x}"))
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct HistoryEntry {
    pub when: i64,
    pub root: PathBuf,
    pub size: u64,
    pub alloc: u64,
    pub files: u64,
    pub dirs: u64,
    pub file: PathBuf,
}

/// Grava um instantâneo só de pastas no histórico daquela raiz.
pub fn add_history(scan: &Scan) -> std::io::Result<HistoryEntry> {
    let dir = history_dir(&scan.root_path);
    fs::create_dir_all(&dir)?;
    let mut s = scan.clone();
    s.root = scan.root.folders_only();
    s.folders_only = true;
    let file = dir.join(format!("{}.{EXT}", scan.when));
    save(&s, &file)?;
    let e = HistoryEntry {
        when: scan.when,
        root: scan.root_path.clone(),
        size: scan.root.size,
        alloc: scan.root.alloc,
        files: scan.root.files,
        dirs: scan.root.dirs,
        file,
    };
    let mut idx = fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(dir.join("indice.jsonl"))?;
    writeln!(
        idx,
        "{}",
        serde_json::to_string(&e).map_err(std::io::Error::other)?
    )?;
    Ok(e)
}

pub fn history(root: &Path) -> Vec<HistoryEntry> {
    let text = fs::read_to_string(history_dir(root).join("indice.jsonl")).unwrap_or_default();
    let mut v: Vec<HistoryEntry> = text
        .lines()
        .filter_map(|l| serde_json::from_str(l).ok())
        .filter(|e: &HistoryEntry| e.file.exists())
        .collect();
    v.sort_by_key(|e| e.when);
    v
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scan::{scan, Progress, ScanOptions};

    #[test]
    fn salvar_carregar_comparar() {
        let t = tempfile::tempdir().unwrap();
        let r = t.path().join("raiz");
        fs::create_dir_all(r.join("cresce")).unwrap();
        fs::create_dir_all(r.join("some")).unwrap();
        fs::write(r.join("cresce/a"), vec![0u8; 1000]).unwrap();
        fs::write(r.join("some/b"), vec![0u8; 300]).unwrap();
        let s1 = scan(&r, &ScanOptions::default(), &Progress::default()).unwrap();
        let f = t.path().join("s1.atlas");
        save(&s1, &f).unwrap();
        let s1 = load(&f).unwrap();
        assert_eq!(s1.root.size, 1300);

        fs::write(r.join("cresce/c"), vec![0u8; 5000]).unwrap();
        fs::remove_dir_all(r.join("some")).unwrap();
        fs::create_dir(r.join("nova")).unwrap();
        let s2 = scan(&r, &ScanOptions::default(), &Progress::default()).unwrap();
        let d = compare(&s1.root, &s2.root, 5, true);
        assert_eq!(d[0].path, Path::new("cresce"));
        assert_eq!(d[0].delta(), 5000);
        assert!(d
            .iter()
            .any(|x| x.path == Path::new("some") && x.status == Status::Removed));
        assert!(d
            .iter()
            .any(|x| x.path == Path::new("nova") && x.status == Status::Added));
        assert!(d
            .iter()
            .any(|x| x.path == Path::new("cresce").join("c") && x.status == Status::Added));
        assert!(locate(&s2, &r.join("cresce")).is_some());
    }
}
