//! Substituto mínimo do `rfd` no Android (onde não há diálogo nativo acessível sem Java):
//! salvar e abrir usam a pasta pública Download/AtlasDisco; escolher pasta devolve essa mesma pasta.

use std::path::PathBuf;

fn base() -> PathBuf {
    let p = PathBuf::from("/storage/emulated/0/Download/AtlasDisco");
    let _ = std::fs::create_dir_all(&p);
    p
}

#[derive(Default)]
pub struct FileDialog {
    file_name: Option<String>,
    exts: Vec<String>,
}

impl FileDialog {
    pub fn new() -> Self {
        Self::default()
    }
    pub fn set_title(self, _t: &str) -> Self {
        self
    }
    pub fn set_file_name(mut self, n: &str) -> Self {
        self.file_name = Some(n.to_string());
        self
    }
    pub fn add_filter(mut self, _name: &str, exts: &[&str]) -> Self {
        self.exts.extend(exts.iter().map(|e| e.to_lowercase()));
        self
    }
    pub fn pick_folder(self) -> Option<PathBuf> {
        Some(base())
    }
    pub fn save_file(self) -> Option<PathBuf> {
        Some(base().join(self.file_name.unwrap_or_else(|| "atlasdisco".into())))
    }
    /// Abre o arquivo mais recente com a extensão pedida em Download/AtlasDisco.
    pub fn pick_file(self) -> Option<PathBuf> {
        std::fs::read_dir(base())
            .ok()?
            .flatten()
            .filter(|e| {
                let p = e.path();
                let ext = p
                    .extension()
                    .map(|x| x.to_string_lossy().to_lowercase())
                    .unwrap_or_default();
                self.exts.is_empty() || self.exts.contains(&ext)
            })
            .max_by_key(|e| e.metadata().and_then(|m| m.modified()).ok())
            .map(|e| e.path())
    }
}
