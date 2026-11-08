//! Busca com vários critérios, conteúdo, caminhos longos e duplicados (tamanho → hash parcial → hash completo).

use crate::analysis::DateKind;
use crate::model::Node;
use crate::pattern::Matcher;
use md5::Md5;
use rayon::prelude::*;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::fs::File;
use std::io::{BufReader, Read};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering::Relaxed};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum Kind {
    #[default]
    Files,
    Folders,
    Both,
}

/// Critérios de busca; salvos como modelo (JSON).
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Criteria {
    pub kind: Kind,
    /// Nome: curingas separados por ';' ou regex.
    pub name: String,
    pub regex: bool,
    /// Trecho que o caminho completo deve conter.
    pub path_contains: String,
    pub min_size: Option<u64>,
    pub max_size: Option<u64>,
    /// Mais antigo que N dias / mais novo que N dias, segundo `date_kind`.
    pub older_than_days: Option<i64>,
    pub newer_than_days: Option<i64>,
    /// 0 = modificação, 1 = último acesso, 2 = criação.
    pub date_kind_code: u8,
    pub min_path_len: Option<usize>,
    pub min_level: Option<usize>,
    pub max_level: Option<usize>,
    /// Atributos obrigatórios (máscara do Windows: H=2, S=4, C=0x800 ...).
    pub attrs: u32,
    /// Texto no conteúdo do arquivo (sem diferenciar maiúsculas, em UTF-8/Latin-1).
    pub content: String,
    /// Não lê arquivos maiores que isto ao buscar conteúdo.
    pub content_max_size: Option<u64>,
    pub max_results: Option<usize>,
}

impl Criteria {
    /// Modelo de busca salvo (JSON).
    pub fn from_json(text: &str) -> Result<Criteria, String> {
        serde_json::from_str(text).map_err(|e| format!("modelo de busca inválido: {e}"))
    }
    pub fn to_json(&self) -> String {
        serde_json::to_string_pretty(self).unwrap_or_default()
    }

    pub fn date_kind(&self) -> DateKind {
        match self.date_kind_code {
            1 => DateKind::Accessed,
            2 => DateKind::Created,
            _ => DateKind::Modified,
        }
    }
}

#[derive(Clone, Debug)]
pub struct Hit {
    pub path: PathBuf,
    pub is_dir: bool,
    pub size: u64,
    pub alloc: u64,
    pub files: u64,
    pub mtime: i64,
    pub atime: i64,
    pub ctime: i64,
    pub attrs: u32,
    pub level: usize,
    /// Grupo de duplicados (0 = sem grupo).
    pub group: usize,
    pub hash: String,
    /// Bytes que só existem na nuvem (não são lidos em buscas de conteúdo nem em duplicados).
    pub cloud: u64,
}

impl Hit {
    fn from(n: &Node, p: &Path, level: usize) -> Hit {
        Hit {
            path: p.to_path_buf(),
            is_dir: n.is_dir,
            size: n.size,
            alloc: n.alloc,
            files: n.files,
            mtime: n.mtime,
            atime: n.atime,
            ctime: n.ctime,
            attrs: n.attrs,
            level,
            group: 0,
            hash: String::new(),
            cloud: n.cloud,
        }
    }
}

/// Tabela para exportar resultados de busca ou de duplicados.
pub fn hits_table(title: String, hits: &[Hit], with_group: bool) -> crate::export::Table {
    use crate::export::Cell;
    let mut headers = vec![
        "Caminho",
        "Tamanho",
        "Alocado",
        "Última modificação",
        "Último acesso",
        "Criação",
        "Atributos",
        "Nível",
    ];
    if with_group {
        headers.splice(0..0, ["Grupo", "Hash"]);
    }
    let rows = hits
        .iter()
        .map(|h| {
            let mut r = vec![
                Cell::Text(h.path.to_string_lossy().into_owned()),
                Cell::Bytes(h.size),
                Cell::Bytes(h.alloc),
                Cell::Date(h.mtime),
                Cell::Date(h.atime),
                Cell::Date(h.ctime),
                Cell::Text(crate::fmt::attrs(h.attrs)),
                Cell::Int(h.level as i64),
            ];
            if with_group {
                r.splice(
                    0..0,
                    [Cell::Int(h.group as i64), Cell::Text(h.hash.clone())],
                );
            }
            r
        })
        .collect();
    crate::export::Table {
        title,
        headers: headers.into_iter().map(String::from).collect(),
        rows,
        indent: vec![],
    }
}

pub fn search(
    root: &Node,
    base: &Path,
    c: &Criteria,
    now: i64,
    cancel: &AtomicBool,
) -> Result<Vec<Hit>, String> {
    let name = Matcher::new(&c.name, c.regex).map_err(|e| e.to_string())?;
    let path_part = c.path_contains.to_lowercase();
    let dk = c.date_kind();
    let mut cand = Vec::new();
    root.walk(base, &mut |n, p, lvl| {
        if lvl == 0 {
            return; // a própria raiz não entra
        }
        match c.kind {
            Kind::Files if n.is_dir => return,
            Kind::Folders if !n.is_dir => return,
            _ => {}
        }
        if let Some(m) = &name {
            if !m.is_match(&n.name) {
                return;
            }
        }
        if c.min_size.is_some_and(|v| n.size < v) || c.max_size.is_some_and(|v| n.size > v) {
            return;
        }
        let t = match dk {
            DateKind::Modified => n.mtime,
            DateKind::Accessed => n.atime,
            DateKind::Created => n.ctime,
        };
        if let Some(d) = c.older_than_days {
            if t <= 0 || now - t < d * 86400 {
                return;
            }
        }
        if let Some(d) = c.newer_than_days {
            if t <= 0 || now - t > d * 86400 {
                return;
            }
        }
        if c.min_level.is_some_and(|v| lvl < v) || c.max_level.is_some_and(|v| lvl > v) {
            return;
        }
        if c.attrs != 0 && n.attrs & c.attrs != c.attrs {
            return;
        }
        let ps = p.to_string_lossy();
        if c.min_path_len.is_some_and(|v| ps.chars().count() < v) {
            return;
        }
        if !path_part.is_empty() && !ps.to_lowercase().contains(&path_part) {
            return;
        }
        cand.push(Hit::from(n, p, lvl));
    });
    if !c.content.is_empty() {
        let needle = c.content.to_lowercase().into_bytes();
        let max = c.content_max_size.unwrap_or(u64::MAX);
        cand = cand
            .into_par_iter()
            .filter(|h| {
                !h.is_dir
                    && h.cloud == 0
                    && h.size <= max
                    && !cancel.load(Relaxed)
                    && file_contains(&h.path, &needle)
            })
            .collect();
    }
    cand.sort_by_key(|x| std::cmp::Reverse(x.size));
    if let Some(m) = c.max_results {
        cand.truncate(m);
    }
    Ok(cand)
}

/// Busca sem diferenciar maiúsculas ASCII, em blocos, com sobreposição para não perder o texto na emenda.
fn file_contains(p: &Path, needle: &[u8]) -> bool {
    let Ok(f) = File::open(p) else { return false };
    let mut r = BufReader::with_capacity(1 << 16, f);
    let mut buf = vec![0u8; 1 << 16];
    let mut carry: Vec<u8> = Vec::new();
    loop {
        let n = match r.read(&mut buf) {
            Ok(0) | Err(_) => return false,
            Ok(n) => n,
        };
        carry.extend(buf[..n].iter().map(|b| b.to_ascii_lowercase()));
        if carry.windows(needle.len()).any(|w| w == needle) {
            return true;
        }
        let keep = needle.len().saturating_sub(1).min(carry.len());
        carry.drain(..carry.len() - keep);
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum DupMode {
    /// Mesmo nome e tamanho (rápido, sem ler arquivos).
    NameSize,
    /// Mesmo conteúdo, conferido por MD5.
    Md5,
    /// Mesmo conteúdo, conferido por SHA-256.
    #[default]
    Sha256,
}

#[derive(Default)]
pub struct DupProgress {
    pub hashed_bytes: AtomicU64,
    pub total_bytes: AtomicU64,
}

/// Grupos de duplicados, do grupo que mais desperdiça espaço para o que menos desperdiça.
pub fn duplicates(
    root: &Node,
    base: &Path,
    min_size: u64,
    mode: DupMode,
    cancel: &AtomicBool,
    prog: &DupProgress,
) -> Vec<Vec<Hit>> {
    let mut by_key: HashMap<(u64, String), Vec<Hit>> = HashMap::new();
    root.walk(base, &mut |n, p, lvl| {
        // Arquivos só na nuvem ficam de fora: calcular o hash baixaria o arquivo inteiro.
        if !n.is_dir && !n.link && !n.hardlink_dup && n.cloud == 0 && n.size >= min_size.max(1) {
            let key = if mode == DupMode::NameSize {
                n.name.to_lowercase()
            } else {
                String::new()
            };
            by_key
                .entry((n.size, key))
                .or_default()
                .push(Hit::from(n, p, lvl));
        }
    });
    let mut groups: Vec<Vec<Hit>> = by_key.into_values().filter(|g| g.len() > 1).collect();
    if mode != DupMode::NameSize {
        prog.total_bytes.store(
            groups.iter().flatten().map(|h| h.size.min(1 << 16)).sum(),
            Relaxed,
        );
        // 1) hash dos primeiros 64 KB; 2) hash completo só de quem empatou.
        groups = split_by(
            groups,
            |h| hash_file(&h.path, mode, Some(1 << 16), cancel, prog),
            cancel,
        );
        prog.total_bytes.fetch_add(
            groups
                .iter()
                .flatten()
                .filter(|h| h.size > 1 << 16)
                .map(|h| h.size)
                .sum(),
            Relaxed,
        );
        groups = split_by(
            groups,
            |h| {
                if h.size <= 1 << 16 {
                    Some(h.hash.clone())
                } else {
                    hash_file(&h.path, mode, None, cancel, prog)
                }
            },
            cancel,
        );
    }
    for (i, g) in groups.iter_mut().enumerate() {
        for h in g.iter_mut() {
            h.group = i + 1;
        }
    }
    groups.sort_by_key(|g| std::cmp::Reverse(wasted(g)));
    for (i, g) in groups.iter_mut().enumerate() {
        for h in g.iter_mut() {
            h.group = i + 1;
        }
    }
    groups
}

pub fn wasted(g: &[Hit]) -> u64 {
    g.first()
        .map(|h| h.size * (g.len() as u64 - 1))
        .unwrap_or(0)
}

fn split_by<F>(groups: Vec<Vec<Hit>>, f: F, cancel: &AtomicBool) -> Vec<Vec<Hit>>
where
    F: Fn(&Hit) -> Option<String> + Sync,
{
    groups
        .into_par_iter()
        .flat_map(|g| {
            let mut m: HashMap<String, Vec<Hit>> = HashMap::new();
            for mut h in g {
                if cancel.load(Relaxed) {
                    break;
                }
                if let Some(k) = f(&h) {
                    h.hash = k.clone();
                    m.entry(k).or_default().push(h);
                }
            }
            m.into_values().filter(|v| v.len() > 1).collect::<Vec<_>>()
        })
        .collect()
}

pub fn hash_file(
    p: &Path,
    mode: DupMode,
    limit: Option<u64>,
    cancel: &AtomicBool,
    prog: &DupProgress,
) -> Option<String> {
    let f = File::open(p).ok()?;
    let mut r: Box<dyn Read> = match limit {
        Some(l) => Box::new(BufReader::with_capacity(1 << 20, f).take(l)),
        None => Box::new(BufReader::with_capacity(1 << 20, f)),
    };
    let mut buf = vec![0u8; 1 << 20];
    let mut md5 = Md5::new();
    let mut sha = Sha256::new();
    loop {
        if cancel.load(Relaxed) {
            return None;
        }
        let n = r.read(&mut buf).ok()?;
        if n == 0 {
            break;
        }
        prog.hashed_bytes.fetch_add(n as u64, Relaxed);
        match mode {
            DupMode::Md5 => md5.update(&buf[..n]),
            _ => sha.update(&buf[..n]),
        }
    }
    Some(match mode {
        DupMode::Md5 => hex(&md5.finalize()),
        _ => hex(&sha.finalize()),
    })
}

fn hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scan::{scan, Progress, ScanOptions};
    use std::fs;

    #[test]
    fn duplicados_e_busca() {
        let t = tempfile::tempdir().unwrap();
        let r = t.path();
        fs::create_dir(r.join("d")).unwrap();
        let big: Vec<u8> = (0..200_000u32).map(|i| (i % 251) as u8).collect();
        fs::write(r.join("a.bin"), &big).unwrap();
        fs::write(r.join("d/b.bin"), &big).unwrap();
        let mut other = big.clone();
        *other.last_mut().unwrap() ^= 1; // mesmo início e tamanho, fim diferente
        fs::write(r.join("d/c.bin"), &other).unwrap();
        fs::write(r.join("nota.txt"), "Olá MUNDO aqui").unwrap();
        let s = scan(r, &ScanOptions::default(), &Progress::default()).unwrap();
        let no = AtomicBool::new(false);
        let g = duplicates(
            &s.root,
            &s.root_path,
            1,
            DupMode::Sha256,
            &no,
            &DupProgress::default(),
        );
        assert_eq!(g.len(), 1);
        assert_eq!(g[0].len(), 2);
        assert!(g[0].iter().all(|h| !h.path.ends_with("c.bin")));
        let g = duplicates(
            &s.root,
            &s.root_path,
            1,
            DupMode::Md5,
            &no,
            &DupProgress::default(),
        );
        assert_eq!(g.len(), 1);

        let c = Criteria {
            content: "mundo".into(),
            ..Default::default()
        };
        let h = search(&s.root, &s.root_path, &c, 0, &no).unwrap();
        assert_eq!(h.len(), 1);
        assert!(h[0].path.ends_with("nota.txt"));
        let c = Criteria {
            name: "*.bin".into(),
            min_size: Some(100_000),
            ..Default::default()
        };
        assert_eq!(search(&s.root, &s.root_path, &c, 0, &no).unwrap().len(), 3);
        let c = Criteria {
            kind: Kind::Folders,
            ..Default::default()
        };
        assert_eq!(search(&s.root, &s.root_path, &c, 0, &no).unwrap().len(), 1);
    }
}
