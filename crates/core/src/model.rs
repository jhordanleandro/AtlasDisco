//! Árvore de arquivos e pastas. Nas pastas, `size`, `alloc`, `files`, `dirs` e `mtime` são totais da subárvore.

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

pub const ATTR_READONLY: u32 = 0x1;
pub const ATTR_HIDDEN: u32 = 0x2;
pub const ATTR_SYSTEM: u32 = 0x4;
pub const ATTR_DIRECTORY: u32 = 0x10;
pub const ATTR_ARCHIVE: u32 = 0x20;
pub const ATTR_TEMPORARY: u32 = 0x100;
pub const ATTR_SPARSE: u32 = 0x200;
pub const ATTR_REPARSE: u32 = 0x400;
pub const ATTR_COMPRESSED: u32 = 0x800;
pub const ATTR_OFFLINE: u32 = 0x1000;
pub const ATTR_ENCRYPTED: u32 = 0x4000;
/// Arquivo de nuvem (OneDrive, etc.) cujo conteúdo só existe online: abrir dispara o download.
pub const ATTR_RECALL_ON_OPEN: u32 = 0x4_0000;
pub const ATTR_RECALL_ON_DATA_ACCESS: u32 = 0x40_0000;
/// "Sempre manter neste dispositivo" / "Liberar espaço" (OneDrive).
pub const ATTR_PINNED: u32 = 0x8_0000;
pub const ATTR_UNPINNED: u32 = 0x10_0000;

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Node {
    pub name: String,
    pub is_dir: bool,
    /// Tamanho lógico (bytes).
    pub size: u64,
    /// Espaço ocupado no disco ("tamanho no disco").
    pub alloc: u64,
    /// Arquivos e subpastas na subárvore (0 para arquivos).
    pub files: u64,
    pub dirs: u64,
    /// Datas em segundos Unix (UTC); 0 = desconhecida.
    pub mtime: i64,
    pub atime: i64,
    pub ctime: i64,
    /// Atributos no formato do Windows (em Unix: só diretório, oculto e link).
    pub attrs: u32,
    /// Bytes que só existem na nuvem (incluídos em `size`, não em `alloc`).
    #[serde(default, skip_serializing_if = "is_zero")]
    pub cloud: u64,
    /// Bytes em fluxos alternativos (NTFS ADS), já incluídos em `size`.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub streams: u64,
    /// Arquivo cujo conteúdo já foi contado por outro hardlink (não soma em `alloc`).
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub hardlink_dup: bool,
    /// Link simbólico/junção não seguido.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub link: bool,
    /// Item informativo, não é um arquivo/pasta real (ex.: espaço de apps no Android). Sem operações.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub synthetic: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub children: Vec<Node>,
}

fn is_zero(v: &u64) -> bool {
    *v == 0
}

impl Node {
    /// Tamanho do que está de fato neste disco (sem o que está só na nuvem).
    pub fn local(&self) -> u64 {
        self.size.saturating_sub(self.cloud)
    }

    /// Arquivo cujo conteúdo não está no disco (placeholder de nuvem ou arquivo offline).
    pub fn is_cloud_only(&self) -> bool {
        !self.is_dir
            && self.attrs & (ATTR_RECALL_ON_DATA_ACCESS | ATTR_RECALL_ON_OPEN | ATTR_OFFLINE) != 0
    }

    pub fn ext(&self) -> String {
        if self.is_dir {
            return String::new();
        }
        Path::new(&self.name)
            .extension()
            .map(|e| e.to_string_lossy().to_lowercase())
            .unwrap_or_default()
    }

    /// Taxa de compressão (0..1) para arquivos compactados/esparsos.
    pub fn compression(&self) -> f64 {
        if self.size == 0 || self.alloc >= self.size {
            0.0
        } else {
            1.0 - self.alloc as f64 / self.size as f64
        }
    }

    /// Ordena filhos recursivamente pelo tamanho (maiores primeiro).
    pub fn sort_by_size(&mut self) {
        self.children
            .sort_by(|a, b| b.size.cmp(&a.size).then_with(|| a.name.cmp(&b.name)));
        for c in &mut self.children {
            c.sort_by_size();
        }
    }

    /// Recalcula totais da subárvore a partir das folhas (após remoção, filtro ou carga).
    pub fn recompute(&mut self) {
        if !self.is_dir {
            return;
        }
        let (mut size, mut alloc, mut files, mut dirs, mut mtime, mut cloud) =
            (0u64, 0u64, 0u64, 0u64, self.mtime, 0u64);
        for c in &mut self.children {
            c.recompute();
            size += c.size;
            alloc += c.alloc;
            cloud += c.cloud;
            if c.is_dir {
                files += c.files;
                dirs += c.dirs + 1;
            } else {
                files += 1;
            }
            mtime = mtime.max(c.mtime);
        }
        self.size = size;
        self.alloc = alloc;
        self.files = files;
        self.dirs = dirs;
        self.mtime = mtime;
        self.cloud = cloud;
    }

    pub fn get(&self, idx: &[usize]) -> Option<&Node> {
        let mut n = self;
        for &i in idx {
            n = n.children.get(i)?;
        }
        Some(n)
    }

    pub fn get_mut(&mut self, idx: &[usize]) -> Option<&mut Node> {
        let mut n = self;
        for &i in idx {
            n = n.children.get_mut(i)?;
        }
        Some(n)
    }

    pub fn child(&self, name: &str) -> Option<&Node> {
        self.children.iter().find(|c| eq_name(&c.name, name))
    }

    /// Visita todos os nós com o caminho completo e o nível relativo.
    pub fn walk<F: FnMut(&Node, &Path, usize)>(&self, base: &Path, f: &mut F) {
        fn rec<F: FnMut(&Node, &Path, usize)>(n: &Node, p: &Path, lvl: usize, f: &mut F) {
            f(n, p, lvl);
            for c in &n.children {
                rec(c, &p.join(&c.name), lvl + 1, f);
            }
        }
        rec(self, base, 0, f);
    }

    /// Cópia só com pastas (para histórico compacto).
    pub fn folders_only(&self) -> Node {
        let mut n = self.clone_shallow();
        n.children = self
            .children
            .iter()
            .filter(|c| c.is_dir)
            .map(|c| c.folders_only())
            .collect();
        n
    }

    pub fn clone_shallow(&self) -> Node {
        Node {
            children: Vec::new(),
            ..self.clone_without_children()
        }
    }

    fn clone_without_children(&self) -> Node {
        Node {
            name: self.name.clone(),
            is_dir: self.is_dir,
            size: self.size,
            alloc: self.alloc,
            cloud: self.cloud,
            files: self.files,
            dirs: self.dirs,
            mtime: self.mtime,
            atime: self.atime,
            ctime: self.ctime,
            attrs: self.attrs,
            streams: self.streams,
            hardlink_dup: self.hardlink_dup,
            link: self.link,
            error: self.error.clone(),
            synthetic: self.synthetic,
            children: Vec::new(),
        }
    }

    /// Remove o nó no índice e atualiza os totais dos ancestrais.
    pub fn remove(&mut self, idx: &[usize]) -> Option<Node> {
        let (last, parent) = idx.split_last()?;
        let removed = {
            let p = self.get_mut(parent)?;
            if *last >= p.children.len() {
                return None;
            }
            p.children.remove(*last)
        };
        let (fc, dc) = if removed.is_dir {
            (removed.files, removed.dirs + 1)
        } else {
            (1, 0)
        };
        let mut n = &mut *self;
        let mut i = 0;
        loop {
            n.size = n.size.saturating_sub(removed.size);
            n.alloc = n.alloc.saturating_sub(removed.alloc);
            n.cloud = n.cloud.saturating_sub(removed.cloud);
            n.files = n.files.saturating_sub(fc);
            n.dirs = n.dirs.saturating_sub(dc);
            if i == parent.len() {
                break;
            }
            n = &mut n.children[parent[i]];
            i += 1;
        }
        Some(removed)
    }
}

/// Comparação de nomes: sem diferenciar maiúsculas no Windows/macOS.
pub fn eq_name(a: &str, b: &str) -> bool {
    if cfg!(target_os = "linux") {
        a == b
    } else {
        a.eq_ignore_ascii_case(b) || a.to_lowercase() == b.to_lowercase()
    }
}

/// Resultado completo de uma varredura.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Scan {
    pub format: u32,
    pub root_path: PathBuf,
    /// Início da varredura (segundos Unix).
    pub when: i64,
    pub seconds: f64,
    pub cluster: u64,
    /// Espaço total e livre do volume, se conhecidos.
    pub volume_total: u64,
    pub volume_free: u64,
    pub folders_only: bool,
    pub errors: Vec<String>,
    pub root: Node,
}

impl Scan {
    pub const FORMAT: u32 = 1;

    /// Caminho completo de um nó dado pelos índices a partir da raiz.
    pub fn path_of(&self, idx: &[usize]) -> PathBuf {
        let mut p = self.root_path.clone();
        let mut n = &self.root;
        for &i in idx {
            n = &n.children[i];
            p.push(&n.name);
        }
        p
    }
}
