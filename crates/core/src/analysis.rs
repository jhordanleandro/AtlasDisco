//! Estatísticas de uma subárvore: tipos de arquivo, idade, maiores arquivos e proprietários.

use crate::model::Node;
use crate::platform;
use rayon::prelude::*;
use std::collections::HashMap;
use std::path::{Path, PathBuf};

#[derive(Clone, Debug, Default)]
pub struct ExtStat {
    pub ext: String,
    pub size: u64,
    pub alloc: u64,
    pub files: u64,
}

pub fn extensions(root: &Node) -> Vec<ExtStat> {
    let mut map: HashMap<String, ExtStat> = HashMap::new();
    root.walk(Path::new(""), &mut |n, _, _| {
        if !n.is_dir && !n.synthetic {
            let e = n.ext();
            let s = map.entry(e.clone()).or_insert_with(|| ExtStat {
                ext: e,
                ..Default::default()
            });
            s.size += n.size;
            s.alloc += n.alloc;
            s.files += 1;
        }
    });
    let mut v: Vec<_> = map.into_values().collect();
    v.sort_by_key(|x| std::cmp::Reverse(x.size));
    v
}

/// Categoria aproximada pela extensão (para cor e agrupamento).
pub fn category(ext: &str) -> &'static str {
    match ext {
        "jpg" | "jpeg" | "png" | "gif" | "bmp" | "tif" | "tiff" | "webp" | "heic" | "raw"
        | "cr2" | "nef" | "svg" | "ico" | "psd" => "Imagens",
        "mp4" | "mkv" | "avi" | "mov" | "wmv" | "flv" | "webm" | "m4v" | "mpg" | "mpeg" | "ts" => {
            "Vídeos"
        }
        "mp3" | "wav" | "flac" | "aac" | "ogg" | "wma" | "m4a" | "opus" => "Áudio",
        "zip" | "rar" | "7z" | "gz" | "xz" | "bz2" | "tar" | "zst" | "cab" => "Compactados",
        "iso" | "img" | "vhd" | "vhdx" | "vmdk" | "qcow2" | "wim" | "esd" => "Imagens de disco",
        "exe" | "dll" | "sys" | "msi" | "so" | "dylib" | "bin" | "app" | "pdb" | "lib" | "a"
        | "o" | "obj" => "Programas",
        "doc" | "docx" | "xls" | "xlsx" | "ppt" | "pptx" | "pdf" | "odt" | "ods" | "odp"
        | "txt" | "rtf" | "md" | "csv" => "Documentos",
        "rs" | "c" | "cpp" | "h" | "hpp" | "cs" | "java" | "py" | "js" | "ts_" | "go" | "html"
        | "css" | "json" | "xml" | "toml" | "yaml" | "yml" => "Código",
        "tmp" | "temp" | "log" | "bak" | "old" | "dmp" | "etl" | "cache" => "Temporários",
        "" => "Sem extensão",
        _ => "Outros",
    }
}

pub fn categories(root: &Node) -> Vec<ExtStat> {
    let mut map: HashMap<&'static str, ExtStat> = HashMap::new();
    for e in extensions(root) {
        let c = category(&e.ext);
        let s = map.entry(c).or_insert_with(|| ExtStat {
            ext: c.into(),
            ..Default::default()
        });
        s.size += e.size;
        s.alloc += e.alloc;
        s.files += e.files;
    }
    let mut v: Vec<_> = map.into_values().collect();
    v.sort_by_key(|x| std::cmp::Reverse(x.size));
    v
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum DateKind {
    #[default]
    Modified,
    Accessed,
    Created,
}

pub const AGE_BUCKETS: [(&str, i64); 8] = [
    ("Menos de 1 mês", 30),
    ("1 a 3 meses", 91),
    ("3 a 6 meses", 182),
    ("6 a 12 meses", 365),
    ("1 a 2 anos", 730),
    ("2 a 5 anos", 1826),
    ("Mais de 5 anos", i64::MAX),
    ("Data desconhecida", -1),
];

#[derive(Clone, Debug, Default)]
pub struct AgeStat {
    pub label: &'static str,
    pub size: u64,
    pub files: u64,
}

pub fn ages(root: &Node, kind: DateKind, now: i64) -> Vec<AgeStat> {
    let mut v: Vec<AgeStat> = AGE_BUCKETS
        .iter()
        .map(|(l, _)| AgeStat {
            label: l,
            ..Default::default()
        })
        .collect();
    root.walk(Path::new(""), &mut |n, _, _| {
        if n.is_dir || n.synthetic {
            return;
        }
        let t = match kind {
            DateKind::Modified => n.mtime,
            DateKind::Accessed => n.atime,
            DateKind::Created => n.ctime,
        };
        let i = if t <= 0 {
            7
        } else {
            let days = (now - t).max(0) / 86400;
            AGE_BUCKETS[..7]
                .iter()
                .position(|(_, d)| days < *d)
                .unwrap_or(6)
        };
        v[i].size += n.size;
        v[i].files += 1;
    });
    v
}

#[derive(Clone, Debug)]
pub struct FileRef {
    pub path: PathBuf,
    pub size: u64,
    pub alloc: u64,
    pub mtime: i64,
    pub atime: i64,
    pub cloud: bool,
}

/// Os `n` maiores arquivos da subárvore.
pub fn largest(root: &Node, base: &Path, n: usize) -> Vec<FileRef> {
    let mut heap = std::collections::BinaryHeap::new(); // min-heap via Reverse
    let mut seq = 0usize;
    let mut paths: HashMap<usize, (PathBuf, u64, i64, i64, bool)> = HashMap::new();
    root.walk(base, &mut |node, p, _| {
        if node.is_dir || node.synthetic {
            return;
        }
        if heap.len() < n {
            heap.push(std::cmp::Reverse((node.size, seq)));
            paths.insert(
                seq,
                (
                    p.to_path_buf(),
                    node.alloc,
                    node.mtime,
                    node.atime,
                    node.cloud > 0,
                ),
            );
            seq += 1;
        } else if let Some(std::cmp::Reverse((min, id))) = heap.peek().copied() {
            if node.size > min {
                heap.pop();
                paths.remove(&id);
                heap.push(std::cmp::Reverse((node.size, seq)));
                paths.insert(
                    seq,
                    (
                        p.to_path_buf(),
                        node.alloc,
                        node.mtime,
                        node.atime,
                        node.cloud > 0,
                    ),
                );
                seq += 1;
            }
        }
    });
    let mut v: Vec<FileRef> = heap
        .into_iter()
        .map(|std::cmp::Reverse((size, id))| {
            let (path, alloc, mtime, atime, cloud) = paths.remove(&id).unwrap();
            FileRef {
                path,
                size,
                alloc,
                mtime,
                atime,
                cloud,
            }
        })
        .collect();
    v.sort_by_key(|x| std::cmp::Reverse(x.size));
    v
}

#[derive(Clone, Debug, Default)]
pub struct OwnerStat {
    pub owner: String,
    pub size: u64,
    pub files: u64,
}

/// Espaço por proprietário. Lê o descritor de segurança de cada arquivo (lento em discos grandes).
pub fn owners(root: &Node, base: &Path, cancel: &std::sync::atomic::AtomicBool) -> Vec<OwnerStat> {
    let mut files = Vec::new();
    root.walk(base, &mut |n, p, _| {
        if !n.is_dir && !n.synthetic {
            files.push((p.to_path_buf(), n.size));
        }
    });
    let map = files
        .par_iter()
        .fold(HashMap::<String, (u64, u64)>::new, |mut m, (p, s)| {
            if !cancel.load(std::sync::atomic::Ordering::Relaxed) {
                let o = platform::owner(p).unwrap_or_else(|| "(sem acesso)".into());
                let e = m.entry(o).or_default();
                e.0 += s;
                e.1 += 1;
            }
            m
        })
        .reduce(HashMap::new, |mut a, b| {
            for (k, (s, f)) in b {
                let e = a.entry(k).or_default();
                e.0 += s;
                e.1 += f;
            }
            a
        });
    let mut v: Vec<OwnerStat> = map
        .into_iter()
        .map(|(owner, (size, files))| OwnerStat { owner, size, files })
        .collect();
    v.sort_by_key(|x| std::cmp::Reverse(x.size));
    v
}

#[cfg(test)]
mod tests {
    use super::*;
    fn f(name: &str, size: u64, mtime: i64) -> Node {
        Node {
            name: name.into(),
            size,
            alloc: size,
            mtime,
            ..Default::default()
        }
    }
    #[test]
    fn estatisticas() {
        let now = 1_000_000_000;
        let mut root = Node {
            is_dir: true,
            name: "r".into(),
            ..Default::default()
        };
        root.children = vec![
            f("a.mp4", 900, now - 10 * 86400),
            f("b.MP4", 100, now - 400 * 86400),
            f("c.txt", 50, 0),
        ];
        root.recompute();
        let e = extensions(&root);
        assert_eq!(e[0].ext, "mp4");
        assert_eq!(e[0].size, 1000);
        assert_eq!(e[0].files, 2);
        let a = ages(&root, DateKind::Modified, now);
        assert_eq!(a[0].size, 900);
        assert_eq!(a[4].size, 100);
        assert_eq!(a[7].size, 50);
        let l = largest(&root, Path::new("r"), 2);
        assert_eq!(l.len(), 2);
        assert_eq!(l[0].size, 900);
        assert_eq!(l[1].path, Path::new("r").join("b.MP4"));
    }
}
