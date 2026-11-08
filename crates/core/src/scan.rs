//! Varredura paralela (rayon). Não segue links simbólicos/junções por padrão, para não contar nada duas vezes.

use crate::model::{Node, Scan};
use crate::pattern::Matcher;
use crate::platform;
use rayon::prelude::*;
use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering::Relaxed};
use std::sync::Mutex;
use std::time::Instant;

#[derive(Clone, Debug)]
pub struct ScanOptions {
    pub threads: usize,
    /// Seguir links simbólicos e junções (pode contar dados em dobro e entrar em ciclos).
    pub follow_links: bool,
    /// Contar uma só vez arquivos com vários hardlinks (abre cada arquivo: mais lento).
    pub dedup_hardlinks: bool,
    /// Somar fluxos alternativos NTFS (ADS).
    pub streams: bool,
    /// Só arquivos que casam (pastas sempre entram).
    pub include: Option<Matcher>,
    /// Arquivos e pastas a ignorar (testado no nome e no caminho completo).
    pub exclude: Option<Matcher>,
    /// Ignorar arquivos que só existem na nuvem (OneDrive sob demanda etc.).
    pub skip_cloud: bool,
}

impl Default for ScanOptions {
    fn default() -> Self {
        let n = std::thread::available_parallelism()
            .map(|n| n.get())
            .unwrap_or(4);
        ScanOptions {
            threads: n.clamp(2, 32),
            follow_links: false,
            dedup_hardlinks: false,
            streams: false,
            include: None,
            exclude: None,
            skip_cloud: false,
        }
    }
}

/// Progresso compartilhado com a interface; `cancel` interrompe a varredura.
#[derive(Default)]
pub struct Progress {
    pub files: AtomicU64,
    pub dirs: AtomicU64,
    pub bytes: AtomicU64,
    pub cancel: AtomicBool,
    pub current: Mutex<String>,
    pub errors: Mutex<Vec<String>>,
    /// Quando ligado, as subpastas de primeiro nível são publicadas em `partial` assim que terminam,
    /// para a interface mostrar resultados durante a varredura.
    pub live: AtomicBool,
    pub partial: Mutex<Partial>,
}

/// Resultado parcial do primeiro nível: arquivos da raiz, pastas prontas e pastas ainda em andamento.
#[derive(Default)]
pub struct Partial {
    pub generation: u64,
    pub files: Vec<Node>,
    pub done: Vec<Node>,
    pub pending: Vec<String>,
}

struct Ctx<'a> {
    opt: &'a ScanOptions,
    p: &'a Progress,
    cluster: u64,
    seen: Mutex<HashSet<(u64, u64)>>,
}

pub fn scan(path: &Path, opt: &ScanOptions, progress: &Progress) -> std::io::Result<Scan> {
    let root_path = normalize(path)?;
    let md = fs::metadata(&root_path)?;
    if !md.is_dir() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "o caminho não é uma pasta",
        ));
    }
    let started = Instant::now();
    let when = chrono::Utc::now().timestamp();
    let ctx = Ctx {
        opt,
        p: progress,
        cluster: platform::cluster_size(&root_path),
        seen: Mutex::new(HashSet::new()),
    };
    let pool = rayon::ThreadPoolBuilder::new()
        .num_threads(opt.threads.max(1))
        .stack_size(16 << 20)
        .thread_name(|i| format!("varredura-{i}"))
        .build()
        .map_err(std::io::Error::other)?;
    let mut root = pool.install(|| scan_dir(&root_path, &md, &ctx, 0));
    root.name = root_path.to_string_lossy().into_owned();
    root.sort_by_size();
    let (volume_total, volume_free) = platform::volume_space(&root_path);
    let errors = std::mem::take(&mut *progress.errors.lock().unwrap());
    Ok(Scan {
        format: Scan::FORMAT,
        root_path,
        when,
        seconds: started.elapsed().as_secs_f64(),
        cluster: ctx.cluster,
        volume_total,
        volume_free,
        folders_only: false,
        errors,
        root,
    })
}

/// Caminho absoluto, sem o prefixo \\?\ que `canonicalize` coloca no Windows.
pub fn normalize(path: &Path) -> std::io::Result<PathBuf> {
    let abs = if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir()?.join(path)
    };
    let s = abs.to_string_lossy();
    let s = s.strip_prefix(r"\\?\").unwrap_or(&s).to_string();
    let mut s = s.replace('/', std::path::MAIN_SEPARATOR_STR);
    // Mantém "C:\" e "/" mas tira barra final do resto.
    while s.len() > 3 && (s.ends_with('\\') || s.ends_with('/')) && s != "/" {
        s.pop();
    }
    if cfg!(windows) && s.len() == 2 && s.ends_with(':') {
        s.push('\\');
    }
    Ok(PathBuf::from(s))
}

fn node_from(name: String, md: &fs::Metadata, is_dir: bool) -> Node {
    let (mtime, atime, ctime) = platform::times(md);
    Node {
        name,
        is_dir,
        mtime,
        atime,
        ctime,
        attrs: platform::attrs(md),
        ..Default::default()
    }
}

fn scan_dir(path: &Path, md: &fs::Metadata, ctx: &Ctx, depth: usize) -> Node {
    let mut dir = node_from(String::new(), md, true);
    if ctx.p.cancel.load(Relaxed) {
        return dir;
    }
    *ctx.p.current.lock().unwrap() = path.to_string_lossy().into_owned();
    let rd = match fs::read_dir(path) {
        Ok(rd) => rd,
        Err(e) => {
            dir.error = Some(e.to_string());
            ctx.p
                .errors
                .lock()
                .unwrap()
                .push(format!("{} — {e}", path.display()));
            return dir;
        }
    };
    let mut files = Vec::new();
    let mut subdirs: Vec<(PathBuf, String, fs::Metadata)> = Vec::new();
    let mut links = Vec::new();
    for entry in rd {
        let entry = match entry {
            Ok(e) => e,
            Err(e) => {
                ctx.p
                    .errors
                    .lock()
                    .unwrap()
                    .push(format!("{} — {e}", path.display()));
                continue;
            }
        };
        let name = entry.file_name().to_string_lossy().into_owned();
        let full = entry.path();
        if let Some(ex) = &ctx.opt.exclude {
            if ex.is_match(&name) || ex.is_match(&full.to_string_lossy()) {
                continue;
            }
        }
        let ft = match entry.file_type() {
            Ok(t) => t,
            Err(_) => continue,
        };
        // No Windows, DirEntry::metadata vem dos dados da listagem (sem chamada extra).
        let emd = match entry.metadata() {
            Ok(m) => m,
            Err(e) => {
                ctx.p
                    .errors
                    .lock()
                    .unwrap()
                    .push(format!("{} — {e}", full.display()));
                continue;
            }
        };
        if ft.is_symlink() {
            let target_dir = fs::metadata(&full).map(|m| m.is_dir()).unwrap_or(false);
            if target_dir {
                if ctx.opt.follow_links {
                    if let Ok(tmd) = fs::metadata(&full) {
                        subdirs.push((full, name, tmd));
                    }
                } else {
                    let mut n = node_from(name, &emd, true);
                    n.link = true;
                    links.push(n);
                }
                continue;
            }
            // Link para arquivo: conta o próprio link (tamanho ~0), como o Explorer.
        }
        if ft.is_dir() {
            subdirs.push((full, name, emd));
            continue;
        }
        if let Some(inc) = &ctx.opt.include {
            if !inc.is_match(&name) {
                continue;
            }
        }
        let mut n = node_from(name, &emd, false);
        n.size = emd.len();
        if n.is_cloud_only() {
            // Só na nuvem: não ocupa disco. Nada aqui abre o arquivo, para não disparar o download.
            if ctx.opt.skip_cloud {
                continue;
            }
            n.cloud = n.size;
            ctx.p.bytes.fetch_add(n.size, Relaxed);
            files.push(n);
            continue;
        }
        n.alloc = if ft.is_symlink() {
            0
        } else {
            platform::allocated(&full, &emd, ctx.cluster)
        };
        if ctx.opt.streams {
            let s = platform::alternate_streams(&full);
            if s > 0 {
                n.streams = s;
                n.size += s;
                n.alloc += s.div_ceil(ctx.cluster) * ctx.cluster;
            }
        }
        if ctx.opt.dedup_hardlinks {
            if let Some(id) = platform::file_id(&full) {
                if id.links > 1 && !ctx.seen.lock().unwrap().insert((id.volume, id.index)) {
                    n.hardlink_dup = true;
                    n.alloc = 0;
                }
            }
        }
        ctx.p.bytes.fetch_add(n.size, Relaxed);
        files.push(n);
    }
    ctx.p.files.fetch_add(files.len() as u64, Relaxed);
    ctx.p
        .dirs
        .fetch_add((subdirs.len() + links.len()) as u64, Relaxed);
    let publish = depth == 0 && ctx.p.live.load(Relaxed);
    if publish {
        let mut part = ctx.p.partial.lock().unwrap();
        part.files = files.clone();
        part.done = links.clone();
        part.pending = subdirs.iter().map(|s| s.1.clone()).collect();
        part.generation += 1;
    }

    let sub: Vec<Node> = subdirs
        .into_par_iter()
        .map(|(p, name, m)| {
            let mut n = scan_dir(&p, &m, ctx, depth + 1);
            n.name = name;
            if publish {
                let mut part = ctx.p.partial.lock().unwrap();
                part.pending.retain(|x| *x != n.name);
                part.done.push(n.clone());
                part.generation += 1;
            }
            n
        })
        .collect();

    dir.children = files;
    dir.children.extend(sub);
    dir.children.extend(links);
    for c in &dir.children {
        dir.size += c.size;
        dir.alloc += c.alloc;
        dir.cloud += c.cloud;
        if c.is_dir {
            dir.files += c.files;
            dir.dirs += c.dirs + 1;
        } else {
            dir.files += 1;
        }
        dir.mtime = dir.mtime.max(c.mtime);
    }
    dir
}

/// Monta uma árvore provisória com o que já terminou; pastas em andamento aparecem vazias e marcadas.
pub fn partial_root(name: &str, part: &Partial) -> Node {
    let mut root = Node {
        name: name.to_string(),
        is_dir: true,
        ..Default::default()
    };
    root.children.extend(part.files.iter().cloned());
    root.children.extend(part.done.iter().cloned());
    for p in &part.pending {
        root.children.push(Node {
            name: p.clone(),
            is_dir: true,
            error: Some("varrendo…".into()),
            ..Default::default()
        });
    }
    root.recompute();
    root.sort_by_size();
    root
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn write(p: &Path, n: usize) {
        fs::File::create(p)
            .unwrap()
            .write_all(&vec![7u8; n])
            .unwrap();
    }

    #[test]
    fn totais_e_filtros() {
        let t = tempfile::tempdir().unwrap();
        let r = t.path();
        fs::create_dir_all(r.join("a/b")).unwrap();
        write(&r.join("x.txt"), 100);
        write(&r.join("a/y.bin"), 5000);
        write(&r.join("a/b/z.mp4"), 70000);
        let p = Progress::default();
        let s = scan(r, &ScanOptions::default(), &p).unwrap();
        assert_eq!(s.root.size, 75100);
        assert_eq!(s.root.files, 3);
        assert_eq!(s.root.dirs, 2);
        assert_eq!(s.root.children[0].name, "a"); // ordenado por tamanho
        assert!(s.root.alloc >= s.root.size);

        let opt = ScanOptions {
            exclude: Matcher::new("*.mp4", false).unwrap(),
            ..Default::default()
        };
        let s = scan(r, &opt, &Progress::default()).unwrap();
        assert_eq!(s.root.size, 5100);

        let opt = ScanOptions {
            include: Matcher::new("*.txt", false).unwrap(),
            ..Default::default()
        };
        let s = scan(r, &opt, &Progress::default()).unwrap();
        assert_eq!(s.root.size, 100);
    }
}
