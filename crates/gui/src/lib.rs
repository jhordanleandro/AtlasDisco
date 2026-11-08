//! AtlasDisco — interface gráfica (egui): Windows, Linux, macOS e Android.

#[cfg(target_os = "android")]
pub mod android;
mod config;
/// No Android não há diálogos nativos de arquivo: este módulo imita a API do rfd usando Download/AtlasDisco.
#[cfg(target_os = "android")]
mod rfd;
mod treemap;
mod views;

use atlasdisco_core::analysis::{self, DateKind, ExtStat, FileRef, OwnerStat};
use atlasdisco_core::export::{self, Format, Table};
use atlasdisco_core::model::eq_name;
use atlasdisco_core::search::{self, Criteria, DupMode, DupProgress, Hit};
use atlasdisco_core::snapshot::{self, Diff, HistoryEntry};
use atlasdisco_core::{fmt, ops, platform, Progress, Scan};
use config::Config;
use eframe::egui::{self, Color32, RichText};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering::Relaxed};
use std::sync::mpsc::{channel, Receiver};
use std::sync::Arc;
use std::time::{Duration, Instant};
use treemap::Measure;

/// Ponto de entrada do desktop: `atlasdisco [pasta|instantâneo.atlas]`.
pub fn desktop_main() -> eframe::Result<()> {
    let start: Option<PathBuf> = std::env::args_os().nth(1).map(PathBuf::from);
    let shot: Option<PathBuf> = std::env::var_os("ATLASDISCO_SCREENSHOT").map(PathBuf::from);
    let opts = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([1360.0, 840.0])
            .with_min_inner_size([760.0, 480.0])
            .with_title("AtlasDisco"),
        ..Default::default()
    };
    run(opts, start, shot)
}

pub fn run(
    opts: eframe::NativeOptions,
    start: Option<PathBuf>,
    shot: Option<PathBuf>,
) -> eframe::Result<()> {
    eframe::run_native(
        "AtlasDisco",
        opts,
        Box::new(move |cc| Ok(Box::new(App::new(cc, start, shot)))),
    )
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Tab {
    /// Árvore de pastas como aba (telas estreitas, ex.: celular).
    Folders,
    Details,
    Treemap,
    Chart,
    Extensions,
    Age,
    Largest,
    Owners,
    Search,
    Duplicates,
    Compare,
    History,
    Errors,
}

impl Tab {
    const ALL: [(Tab, &'static str); 12] = [
        (Tab::Details, "Detalhes"),
        (Tab::Treemap, "Mapa"),
        (Tab::Chart, "Gráfico"),
        (Tab::Extensions, "Tipos de arquivo"),
        (Tab::Age, "Idade"),
        (Tab::Largest, "Maiores arquivos"),
        (Tab::Owners, "Proprietários"),
        (Tab::Search, "Busca"),
        (Tab::Duplicates, "Duplicados"),
        (Tab::Compare, "Comparação"),
        (Tab::History, "Histórico"),
        (Tab::Errors, "Erros"),
    ];
}

/// Ações pedidas pela interface, aplicadas no fim do quadro (evita conflito de empréstimos).
pub enum Act {
    Select(Vec<usize>),
    Scan(PathBuf),
    Reveal(PathBuf),
    Open(PathBuf),
    Copy(String),
    Trash(Vec<PathBuf>),
    AskDelete(Vec<PathBuf>),
    Delete(Vec<PathBuf>),
    Compress(PathBuf, bool),
    Hardlink(Vec<(PathBuf, Vec<PathBuf>)>),
    Transfer(Vec<PathBuf>, bool),
    Rename(Vec<PathBuf>),
}

struct ScanJob {
    progress: Arc<Progress>,
    rx: Receiver<std::io::Result<Scan>>,
    started: Instant,
    path: PathBuf,
}

enum BgResult {
    Owners(Vec<OwnerStat>),
    Search(Result<Vec<Hit>, String>),
    Dups(Vec<Vec<Hit>>),
    Ops(String, ops::Report, Vec<PathBuf>),
    Text(Result<String, String>),
}

struct Bg {
    rx: Receiver<BgResult>,
    cancel: Arc<AtomicBool>,
    label: String,
    dup: Option<Arc<DupProgress>>,
    started: Instant,
}

/// Coluna da visão de detalhes.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Col {
    Name,
    Size,
    Alloc,
    Pct,
    Files,
    Dirs,
    Modified,
    Accessed,
    Created,
    Attrs,
    Ext,
    Compression,
    Avg,
    PathLen,
    Owner,
    Cloud,
}

impl Col {
    pub const ALL: [(Col, &'static str); 16] = [
        (Col::Name, "Nome"),
        (Col::Size, "Tamanho"),
        (Col::Alloc, "Alocado"),
        (Col::Pct, "% do pai"),
        (Col::Files, "Arquivos"),
        (Col::Dirs, "Pastas"),
        (Col::Modified, "Última modificação"),
        (Col::Accessed, "Último acesso"),
        (Col::Created, "Criação"),
        (Col::Attrs, "Atributos"),
        (Col::Ext, "Tipo"),
        (Col::Compression, "Compr."),
        (Col::Avg, "Tamanho médio"),
        (Col::PathLen, "Compr. caminho"),
        (Col::Owner, "Proprietário"),
        (Col::Cloud, "Na nuvem"),
    ];
}

type DetailsKey = (u64, Vec<usize>, usize, bool, String, Measure, bool);
type TilesKey = (u64, Vec<usize>, [i32; 4], usize, Measure, bool, bool);

#[derive(Default)]
pub struct Caches {
    pub key: Option<(u64, Vec<usize>)>,
    pub ext: Option<Vec<ExtStat>>,
    pub cats: Option<Vec<ExtStat>>,
    pub ages: Option<(DateKind, Vec<analysis::AgeStat>)>,
    pub top: Option<Vec<FileRef>>,
    pub details: Option<(DetailsKey, Vec<usize>)>,
    pub tiles: Option<(TilesKey, Vec<treemap::Tile>)>,
    pub owners_of: std::collections::HashMap<PathBuf, String>,
}

pub struct SearchUi {
    pub crit: Criteria,
    pub name: String,
    pub min: String,
    pub max: String,
    pub older: String,
    pub newer: String,
    pub pathlen: String,
    pub hits: Vec<Hit>,
    pub marked: Vec<bool>,
}

pub struct DupUi {
    pub min: String,
    pub mode: DupMode,
    pub hits: Vec<Hit>,
    pub marked: Vec<bool>,
    pub groups: usize,
    pub wasted: u64,
}

pub struct CompareUi {
    pub old: Option<Arc<Scan>>,
    pub label: String,
    pub depth: usize,
    pub files: bool,
    pub diffs: Vec<Diff>,
    pub dirty: bool,
}

pub struct App {
    pub cfg: Config,
    pub path_input: String,
    pub scan: Option<Arc<Scan>>,
    pub gen: u64,
    job: Option<ScanJob>,
    bg: Option<Bg>,
    pub sel: Vec<usize>,
    pub detail_sel: std::collections::BTreeSet<usize>,
    pub expand_to_sel: bool,
    pub tab: Tab,
    pub measure: Measure,
    pub unit: fmt::Unit,
    pub filter: String,
    pub sort_col: usize,
    pub sort_asc: bool,
    pub date_kind: DateKind,
    pub owners: Option<Vec<OwnerStat>>,
    pub search: SearchUi,
    pub dup: DupUi,
    pub cmp: CompareUi,
    pub history: Vec<HistoryEntry>,
    pub caches: Caches,
    pub status: String,
    pub chart_pie: bool,
    pub tm_files: bool,
    pub tm_hierarchy: bool,
    show_options: bool,
    show_schedule: bool,
    show_about: bool,
    confirm: Option<(String, Vec<PathBuf>)>,
    rename: Option<(Vec<PathBuf>, String, String)>,
    sched: config::Sched,
    screenshot: Option<(PathBuf, u32)>,
    pub compact: bool,
}

impl App {
    fn new(cc: &eframe::CreationContext, start: Option<PathBuf>, shot: Option<PathBuf>) -> App {
        let cfg = Config::load();
        cc.egui_ctx.set_visuals(if cfg.dark {
            egui::Visuals::dark()
        } else {
            egui::Visuals::light()
        });
        let mut app = App {
            path_input: start
                .as_ref()
                .map(|p| p.to_string_lossy().into_owned())
                .or_else(|| cfg.recent.first().cloned())
                .unwrap_or_default(),
            cfg,
            scan: None,
            gen: 0,
            job: None,
            bg: None,
            sel: vec![],
            detail_sel: Default::default(),
            expand_to_sel: false,
            tab: Tab::Details,
            measure: Measure::Local,
            unit: fmt::Unit::Auto,
            filter: String::new(),
            sort_col: 3,
            sort_asc: false,
            date_kind: DateKind::Modified,
            owners: None,
            search: SearchUi {
                crit: Criteria::default(),
                name: String::new(),
                min: String::new(),
                max: String::new(),
                older: String::new(),
                newer: String::new(),
                pathlen: String::new(),
                hits: vec![],
                marked: vec![],
            },
            dup: DupUi {
                min: "1 MB".into(),
                mode: DupMode::Sha256,
                hits: vec![],
                marked: vec![],
                groups: 0,
                wasted: 0,
            },
            cmp: CompareUi {
                old: None,
                label: String::new(),
                depth: 4,
                files: false,
                diffs: vec![],
                dirty: false,
            },
            history: vec![],
            caches: Caches::default(),
            status: String::new(),
            chart_pie: true,
            tm_files: true,
            tm_hierarchy: true,
            show_options: false,
            show_schedule: false,
            show_about: false,
            confirm: None,
            rename: None,
            sched: config::Sched::default(),
            screenshot: shot.map(|p| (p, 0)),
            compact: false,
        };
        // ATLASDISCO_ABA=n abre direto na aba n (usado nas capturas de tela de teste).
        if let Some(n) = std::env::var("ATLASDISCO_ABA")
            .ok()
            .and_then(|v| v.parse::<usize>().ok())
        {
            app.tab = Tab::ALL[n.min(Tab::ALL.len() - 1)].0;
        }
        if let Some(p) = start {
            // Um instantâneo .atlas abre direto (duplo clique no arquivo); uma pasta é varrida.
            if p.extension()
                .is_some_and(|e| e.eq_ignore_ascii_case(snapshot::EXT))
            {
                match snapshot::load(&p) {
                    Ok(s) => {
                        app.history = snapshot::history(&s.root_path);
                        app.set_scan(s);
                    }
                    Err(e) => app.status = format!("Erro ao abrir {}: {e}", p.display()),
                }
            } else {
                app.start_scan(p);
            }
        }
        app
    }

    pub fn sz(&self, b: u64) -> String {
        fmt::size_in(b, self.unit, self.cfg.decimals)
    }

    pub fn val(&self, n: &atlasdisco_core::Node) -> u64 {
        treemap::value(n, self.measure)
    }

    pub fn val_str(&self, v: u64) -> String {
        if self.measure == Measure::Files {
            format!("{} arq.", fmt::int(v as i64))
        } else {
            self.sz(v)
        }
    }

    fn start_scan(&mut self, path: PathBuf) {
        if self.job.is_some() {
            return;
        }
        let opt = match self.cfg.scan_options() {
            Ok(o) => o,
            Err(e) => {
                self.status = format!("Filtro inválido: {e}");
                return;
            }
        };
        let progress = Arc::new(Progress::default());
        let (tx, rx) = channel();
        let p2 = progress.clone();
        let path2 = path.clone();
        std::thread::spawn(move || {
            let r = atlasdisco_core::scan(&path2, &opt, &p2);
            let _ = tx.send(r);
        });
        self.path_input = path.to_string_lossy().into_owned();
        self.job = Some(ScanJob {
            progress,
            rx,
            started: Instant::now(),
            path,
        });
        self.status.clear();
    }

    fn finish_scan(&mut self, s: Scan) {
        self.cfg.add_recent(&s.root_path.to_string_lossy());
        if self.cfg.history && !s.root.children.is_empty() {
            if let Err(e) = snapshot::add_history(&s) {
                self.status = format!("Histórico não gravado: {e}");
            }
        }
        self.history = snapshot::history(&s.root_path);
        self.set_scan(s);
    }

    fn set_scan(&mut self, s: Scan) {
        self.path_input = s.root_path.to_string_lossy().into_owned();
        self.scan = Some(Arc::new(s));
        self.gen += 1;
        self.sel.clear();
        self.detail_sel.clear();
        self.caches = Caches::default();
        self.owners = None;
        self.cmp.dirty = true;
        self.search.hits.clear();
        self.dup.hits.clear();
    }

    fn spawn_bg(
        &mut self,
        label: &str,
        dup: Option<Arc<DupProgress>>,
        f: impl FnOnce(Arc<AtomicBool>) -> BgResult + Send + 'static,
    ) {
        if self.bg.is_some() {
            self.status = "Aguarde a tarefa em andamento terminar.".into();
            return;
        }
        let cancel = Arc::new(AtomicBool::new(false));
        let c2 = cancel.clone();
        let (tx, rx) = channel();
        std::thread::spawn(move || {
            let _ = tx.send(f(c2));
        });
        self.bg = Some(Bg {
            rx,
            cancel,
            label: label.into(),
            dup,
            started: Instant::now(),
        });
    }

    pub fn run_owners(&mut self) {
        let Some(scan) = self.scan.clone() else {
            return;
        };
        let sel = self.sel.clone();
        self.spawn_bg("Lendo proprietários", None, move |cancel| {
            let node = scan.root.get(&sel).unwrap_or(&scan.root);
            BgResult::Owners(analysis::owners(node, &scan.path_of(&sel), &cancel))
        });
    }

    pub fn run_search(&mut self) {
        let Some(scan) = self.scan.clone() else {
            return;
        };
        let s = &mut self.search;
        let num = |t: &str| t.trim().parse::<i64>().ok();
        s.crit.name = s.name.clone();
        s.crit.min_size = fmt::parse_size(&s.min);
        s.crit.max_size = fmt::parse_size(&s.max);
        s.crit.older_than_days = num(&s.older);
        s.crit.newer_than_days = num(&s.newer);
        s.crit.min_path_len = num(&s.pathlen).map(|v| v.max(0) as usize);
        let crit = s.crit.clone();
        let sel = self.sel.clone();
        self.spawn_bg("Buscando", None, move |cancel| {
            let node = scan.root.get(&sel).unwrap_or(&scan.root);
            BgResult::Search(search::search(
                node,
                &scan.path_of(&sel),
                &crit,
                chrono::Utc::now().timestamp(),
                &cancel,
            ))
        });
    }

    pub fn run_dups(&mut self) {
        let Some(scan) = self.scan.clone() else {
            return;
        };
        let min = fmt::parse_size(&self.dup.min).unwrap_or(0);
        let mode = self.dup.mode;
        let sel = self.sel.clone();
        let prog = Arc::new(DupProgress::default());
        let p2 = prog.clone();
        self.spawn_bg("Procurando duplicados", Some(prog), move |cancel| {
            let node = scan.root.get(&sel).unwrap_or(&scan.root);
            BgResult::Dups(search::duplicates(
                node,
                &scan.path_of(&sel),
                min,
                mode,
                &cancel,
                &p2,
            ))
        });
    }

    pub fn export_table(&mut self, t: Table, f: Format) {
        let name = format!(
            "atlasdisco-{}.{}",
            chrono::Local::now().format("%Y-%m-%d_%H%M"),
            f.ext()
        );
        if let Some(path) = rfd::FileDialog::new()
            .set_file_name(&name)
            .add_filter(f.name(), &[f.ext()])
            .save_file()
        {
            self.status = match export::write(&t, &path, f) {
                Ok(()) => format!("Exportado: {}", path.display()),
                Err(e) => format!("Erro ao exportar: {e}"),
            };
        }
    }

    /// Índices do nó correspondente a um caminho absoluto.
    pub fn idx_of(&self, path: &Path) -> Option<Vec<usize>> {
        let scan = self.scan.as_ref()?;
        let root: Vec<_> = scan.root_path.components().collect();
        let all: Vec<_> = path.components().collect();
        if all.len() < root.len() {
            return None;
        }
        let mut idx = vec![];
        let mut n = &scan.root;
        for part in &all[root.len()..] {
            let name = part.as_os_str().to_string_lossy();
            let i = n.children.iter().position(|c| eq_name(&c.name, &name))?;
            idx.push(i);
            n = &n.children[i];
        }
        Some(idx)
    }

    /// Tira da árvore itens apagados/movidos e ajusta a seleção.
    fn remove_paths(&mut self, paths: &[PathBuf]) {
        let mut idxs: Vec<Vec<usize>> = paths
            .iter()
            .filter_map(|p| self.idx_of(p))
            .filter(|i| !i.is_empty())
            .collect();
        if idxs.is_empty() {
            return;
        }
        // Remove do índice maior para o menor, para não invalidar os demais.
        idxs.sort();
        idxs.reverse();
        let Some(arc) = self.scan.as_mut() else {
            return;
        };
        let scan = Arc::make_mut(arc);
        for i in &idxs {
            scan.root.remove(i);
            if self.sel.starts_with(i) {
                self.sel = i[..i.len() - 1].to_vec();
            }
        }
        self.gen += 1;
        self.caches = Caches::default();
        self.detail_sel.clear();
        let gone = |p: &PathBuf| paths.iter().any(|x| p.starts_with(x));
        let keep: Vec<bool> = self.search.hits.iter().map(|h| !gone(&h.path)).collect();
        retain_marked(&mut self.search.hits, &mut self.search.marked, &keep);
        let keep: Vec<bool> = self.dup.hits.iter().map(|h| !gone(&h.path)).collect();
        retain_marked(&mut self.dup.hits, &mut self.dup.marked, &keep);
    }

    fn apply(&mut self, ctx: &egui::Context, acts: Vec<Act>) {
        for a in acts {
            match a {
                Act::Select(i) => {
                    if self.sel != i {
                        self.sel = i;
                        self.detail_sel.clear();
                        self.expand_to_sel = true;
                    }
                }
                Act::Scan(p) => self.start_scan(p),
                Act::Reveal(p) => {
                    let _ = platform::reveal(&p);
                }
                Act::Open(p) => {
                    let _ = platform::open(&p);
                }
                Act::Copy(s) => ctx.copy_text(s),
                Act::Trash(v) => self.spawn_bg("Enviando para a lixeira", None, move |_| {
                    let r = ops::to_trash(&v);
                    BgResult::Ops("Lixeira".into(), r, v)
                }),
                Act::AskDelete(v) => {
                    let msg = if v.len() == 1 {
                        format!(
                            "Excluir definitivamente \"{}\"? Não vai para a lixeira.",
                            v[0].display()
                        )
                    } else {
                        format!(
                            "Excluir definitivamente {} itens? Não vão para a lixeira.",
                            v.len()
                        )
                    };
                    self.confirm = Some((msg, v));
                }
                Act::Delete(v) => self.spawn_bg("Excluindo", None, move |_| {
                    let r = ops::delete_permanently(&v);
                    BgResult::Ops("Exclusão".into(), r, v)
                }),
                Act::Compress(p, on) => self.spawn_bg(
                    if on {
                        "Compactando (NTFS)"
                    } else {
                        "Descompactando (NTFS)"
                    },
                    None,
                    move |_| BgResult::Text(ops::ntfs_compress(&p, on)),
                ),
                Act::Hardlink(groups) => {
                    self.spawn_bg("Substituindo por hardlinks", None, move |_| {
                        let mut total = ops::Report::default();
                        for (keep, copies) in &groups {
                            let r = ops::hardlink_group(keep, copies);
                            total.ok += r.ok;
                            total.freed += r.freed;
                            total.failed.extend(r.failed);
                        }
                        BgResult::Ops("Deduplicação por hardlink".into(), total, vec![])
                    })
                }
                Act::Transfer(v, mov) => {
                    if let Some(dest) = rfd::FileDialog::new()
                        .set_title(if mov { "Mover para" } else { "Copiar para" })
                        .pick_folder()
                    {
                        let base = self.scan.as_ref().map(|s| s.path_of(&self.sel));
                        self.spawn_bg(if mov { "Movendo" } else { "Copiando" }, None, move |_| {
                            let r = ops::transfer(&v, &dest, base.as_deref(), mov);
                            BgResult::Ops(
                                if mov { "Mover".into() } else { "Copiar".into() },
                                r,
                                if mov { v } else { vec![] },
                            )
                        });
                    }
                }
                Act::Rename(v) => self.rename = Some((v, String::new(), String::new())),
            }
        }
    }

    fn poll(&mut self, ctx: &egui::Context) {
        if let Some(job) = &self.job {
            match job.rx.try_recv() {
                Ok(Ok(s)) => {
                    let cancelled = job.progress.cancel.load(Relaxed);
                    self.job = None;
                    self.finish_scan(s);
                    if cancelled {
                        self.status = "Varredura interrompida: os totais estão incompletos.".into();
                    }
                }
                Ok(Err(e)) => {
                    self.status = format!("Erro ao varrer {}: {e}", job.path.display());
                    self.job = None;
                }
                Err(_) => ctx.request_repaint_after(Duration::from_millis(150)),
            }
        }
        if let Some(bg) = &self.bg {
            match bg.rx.try_recv() {
                Ok(r) => {
                    let secs = bg.started.elapsed().as_secs_f32();
                    self.bg = None;
                    match r {
                        BgResult::Owners(o) => self.owners = Some(o),
                        BgResult::Search(Ok(h)) => {
                            self.status =
                                format!("{} resultados em {secs:.1} s", fmt::int(h.len() as i64));
                            self.search.marked = vec![false; h.len()];
                            self.search.hits = h;
                        }
                        BgResult::Search(Err(e)) => self.status = format!("Busca: {e}"),
                        BgResult::Dups(g) => {
                            self.dup.groups = g.len();
                            self.dup.wasted = g.iter().map(|x| search::wasted(x)).sum();
                            self.dup.hits = g.into_iter().flatten().collect();
                            self.dup.marked = vec![false; self.dup.hits.len()];
                            self.status =
                                format!("{} grupos de duplicados em {secs:.1} s", self.dup.groups);
                        }
                        BgResult::Ops(what, rep, paths) => {
                            let failed: Vec<PathBuf> =
                                rep.failed.iter().map(|f| f.0.clone()).collect();
                            let done: Vec<PathBuf> =
                                paths.into_iter().filter(|p| !failed.contains(p)).collect();
                            self.remove_paths(&done);
                            self.status = format!(
                                "{what}: {} itens, {} liberados{}",
                                rep.ok,
                                self.sz(rep.freed),
                                if rep.failed.is_empty() {
                                    String::new()
                                } else {
                                    format!(
                                        "; {} falharam (ex.: {} — {})",
                                        rep.failed.len(),
                                        rep.failed[0].0.display(),
                                        rep.failed[0].1
                                    )
                                }
                            );
                        }
                        BgResult::Text(r) => {
                            self.status = match r {
                                Ok(t) => format!(
                                    "Concluído. {}",
                                    t.lines().rfind(|l| !l.trim().is_empty()).unwrap_or("")
                                ),
                                Err(e) => format!("Erro: {}", e.trim()),
                            }
                        }
                    }
                }
                Err(_) => ctx.request_repaint_after(Duration::from_millis(150)),
            }
        }
    }

    fn menus(&mut self, ui: &mut egui::Ui, ctx: &egui::Context, acts: &mut Vec<Act>) {
        egui::menu::bar(ui, |ui| {
            ui.menu_button("Arquivo", |ui| {
                if ui.button("Escolher pasta…").clicked() {
                    ui.close_menu();
                    if let Some(p) = rfd::FileDialog::new().pick_folder() {
                        acts.push(Act::Scan(p));
                    }
                }
                ui.menu_button("Recentes", |ui| {
                    for r in self.cfg.recent.clone() {
                        if ui.button(&r).clicked() {
                            ui.close_menu();
                            acts.push(Act::Scan(PathBuf::from(r)));
                        }
                    }
                });
                ui.separator();
                if ui.button("Abrir instantâneo (.atlas)…").clicked() {
                    ui.close_menu();
                    if let Some(p) = rfd::FileDialog::new()
                        .add_filter("Instantâneo AtlasDisco", &["atlas"])
                        .pick_file()
                    {
                        match snapshot::load(&p) {
                            Ok(s) => {
                                self.history = snapshot::history(&s.root_path);
                                self.set_scan(s);
                                self.status = format!("Instantâneo carregado: {}", p.display());
                            }
                            Err(e) => self.status = format!("Erro ao abrir: {e}"),
                        }
                    }
                }
                let has = self.scan.is_some();
                if ui
                    .add_enabled(has, egui::Button::new("Salvar instantâneo…"))
                    .clicked()
                {
                    ui.close_menu();
                    let name = format!(
                        "atlasdisco-{}.atlas",
                        chrono::Local::now().format("%Y-%m-%d_%H%M")
                    );
                    if let (Some(s), Some(p)) = (
                        self.scan.clone(),
                        rfd::FileDialog::new()
                            .set_file_name(&name)
                            .add_filter("Instantâneo", &["atlas"])
                            .save_file(),
                    ) {
                        self.status = match snapshot::save(&s, &p) {
                            Ok(()) => format!("Instantâneo salvo: {}", p.display()),
                            Err(e) => format!("Erro: {e}"),
                        };
                    }
                }
                ui.add_enabled_ui(has, |ui| {
                    ui.menu_button("Exportar seleção", |ui| {
                        ui.horizontal(|ui| {
                            ui.label("Níveis:");
                            ui.add(egui::DragValue::new(&mut self.cfg.export_depth).range(0..=64));
                            ui.checkbox(&mut self.cfg.export_files, "com arquivos");
                        });
                        for f in Format::ALL {
                            if ui.button(f.name()).clicked() {
                                ui.close_menu();
                                if let Some(s) = self.scan.clone() {
                                    let node = s.root.get(&self.sel).unwrap_or(&s.root);
                                    let mut t = export::tree_table(
                                        node,
                                        &s.path_of(&self.sel),
                                        self.cfg.export_depth,
                                        self.cfg.export_files,
                                    );
                                    t.title = export::summary(&s);
                                    self.export_table(t, f);
                                }
                            }
                        }
                    });
                });
                ui.separator();
                if ui.button("Agendar varredura…").clicked() {
                    ui.close_menu();
                    self.sched.path = self.path_input.clone();
                    self.show_schedule = true;
                }
                if ui.button("Nova janela").clicked() {
                    ui.close_menu();
                    if let Ok(exe) = std::env::current_exe() {
                        let _ = std::process::Command::new(exe).spawn();
                    }
                }
                #[cfg(windows)]
                if ui.button("Reiniciar como administrador").clicked() {
                    ui.close_menu();
                    if config::restart_as_admin(&self.path_input) {
                        ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                    }
                }
                ui.separator();
                if ui.button("Sair").clicked() {
                    ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                }
            });
            ui.menu_button("Exibir", |ui| {
                ui.label("Medida");
                ui.radio_value(
                    &mut self.measure,
                    Measure::Local,
                    "Tamanho local (sem nuvem)",
                );
                ui.radio_value(
                    &mut self.measure,
                    Measure::Size,
                    "Tamanho total (com nuvem)",
                );
                ui.radio_value(&mut self.measure, Measure::Alloc, "Espaço alocado");
                ui.radio_value(&mut self.measure, Measure::Files, "Número de arquivos");
                ui.separator();
                ui.label("Unidade");
                for (u, l) in [
                    (fmt::Unit::Auto, "Automática"),
                    (fmt::Unit::KB, "KB"),
                    (fmt::Unit::MB, "MB"),
                    (fmt::Unit::GB, "GB"),
                    (fmt::Unit::TB, "TB"),
                ] {
                    ui.radio_value(&mut self.unit, u, l);
                }
                ui.horizontal(|ui| {
                    ui.label("Casas decimais");
                    ui.add(egui::DragValue::new(&mut self.cfg.decimals).range(0..=3));
                });
                ui.separator();
                ui.checkbox(&mut self.cfg.hide_empty, "Ocultar pastas vazias");
                ui.checkbox(&mut self.cfg.files_in_tree, "Arquivos na árvore");
                ui.menu_button("Colunas dos detalhes", |ui| {
                    for (k, (_, l)) in Col::ALL.iter().enumerate().skip(1) {
                        ui.checkbox(&mut self.cfg.cols[k], *l);
                    }
                    if ui.button("Restaurar padrão").clicked() {
                        self.cfg.cols = Config::default().cols;
                    }
                });
                ui.separator();
                if ui.checkbox(&mut self.cfg.dark, "Tema escuro").changed() {
                    ctx.set_visuals(if self.cfg.dark {
                        egui::Visuals::dark()
                    } else {
                        egui::Visuals::light()
                    });
                }
            });
            ui.menu_button("Ferramentas", |ui| {
                for (t, l) in [
                    (Tab::Search, "Buscar arquivos"),
                    (Tab::Duplicates, "Arquivos duplicados"),
                    (Tab::Compare, "Comparar varreduras"),
                    (Tab::History, "Histórico"),
                    (Tab::Owners, "Proprietários"),
                ] {
                    if ui.button(l).clicked() {
                        ui.close_menu();
                        self.tab = t;
                    }
                }
                ui.separator();
                if ui.button("Opções de varredura…").clicked() {
                    ui.close_menu();
                    self.show_options = true;
                }
                #[cfg(windows)]
                {
                    ui.separator();
                    if ui
                        .button("Adicionar ao menu de contexto do Explorer")
                        .clicked()
                    {
                        ui.close_menu();
                        self.status = config::explorer_menu(true);
                    }
                    if ui
                        .button("Remover do menu de contexto do Explorer")
                        .clicked()
                    {
                        ui.close_menu();
                        self.status = config::explorer_menu(false);
                    }
                }
            });
            ui.menu_button("Ajuda", |ui| {
                if ui.button("Sobre o AtlasDisco").clicked() {
                    ui.close_menu();
                    self.show_about = true;
                }
            });
        });
    }

    fn toolbar(&mut self, ui: &mut egui::Ui, acts: &mut Vec<Act>) {
        // Em tela estreita os controles quebram linha em vez de sair da tela.
        ui.horizontal_wrapped(|ui| {
            egui::ComboBox::from_id_salt("unidades")
                .selected_text("Locais")
                .width(90.0)
                .show_ui(ui, |ui| {
                    ui.set_min_width(300.0);
                    let locs = platform::locations();
                    for (title, cloud) in [
                        ("Discos e unidades", false),
                        ("Nuvem (arquivos sob demanda contados à parte)", true),
                    ] {
                        let group: Vec<_> = locs
                            .iter()
                            .filter(|l| (l.kind == platform::LocKind::Cloud) == cloud)
                            .collect();
                        if group.is_empty() {
                            continue;
                        }
                        ui.label(RichText::new(title).weak().small());
                        for l in group {
                            let text = if cloud {
                                format!("Nuvem: {}  —  {}", l.label, l.path)
                            } else {
                                format!("{}  {}  ({})", l.path, l.label, l.kind.label())
                            };
                            if ui.button(text).clicked() {
                                acts.push(Act::Scan(PathBuf::from(&l.path)));
                            }
                        }
                        ui.separator();
                    }
                });
            if cfg!(not(target_os = "android"))
                && ui.button("📁").on_hover_text("Escolher pasta").clicked()
            {
                if let Some(p) = rfd::FileDialog::new().pick_folder() {
                    acts.push(Act::Scan(p));
                }
            }
            let w = if self.compact {
                ui.available_width().max(160.0)
            } else {
                (ui.available_width() - 520.0).max(200.0)
            };
            let r = ui.add(
                egui::TextEdit::singleline(&mut self.path_input)
                    .desired_width(w)
                    .hint_text("Pasta, unidade ou caminho de rede (\\\\servidor\\pasta)"),
            );
            let enter = r.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
            let scanning = self.job.is_some();
            if ui
                .add_enabled(!scanning, egui::Button::new("▶ Varrer"))
                .clicked()
                || (enter && !scanning)
            {
                acts.push(Act::Scan(PathBuf::from(self.path_input.trim())));
            }
            if ui
                .add_enabled(scanning, egui::Button::new("⏹ Parar"))
                .clicked()
            {
                if let Some(j) = &self.job {
                    j.progress.cancel.store(true, Relaxed);
                }
            }
            if ui
                .add_enabled(!scanning && self.scan.is_some(), egui::Button::new("⟳"))
                .on_hover_text("Atualizar")
                .clicked()
            {
                if let Some(s) = &self.scan {
                    acts.push(Act::Scan(s.root_path.clone()));
                }
            }
            if ui
                .add_enabled(!self.sel.is_empty(), egui::Button::new("⬆"))
                .on_hover_text("Pasta acima")
                .clicked()
            {
                let mut p = self.sel.clone();
                p.pop();
                acts.push(Act::Select(p));
            }
            ui.separator();
            ui.label("🔎");
            ui.add(
                egui::TextEdit::singleline(&mut self.filter)
                    .desired_width(160.0)
                    .hint_text("Filtrar em tempo real"),
            );
            if !self.filter.is_empty() && ui.small_button("✖").clicked() {
                self.filter.clear();
            }
        });
    }

    fn status_bar(&mut self, ui: &mut egui::Ui) {
        ui.horizontal_wrapped(|ui| {
            if let Some(j) = &self.job {
                ui.spinner();
                let p = &j.progress;
                ui.label(format!(
                    "Varrendo… {} arquivos, {} pastas, {} — {:.0} s",
                    fmt::int(p.files.load(Relaxed) as i64),
                    fmt::int(p.dirs.load(Relaxed) as i64),
                    fmt::size(p.bytes.load(Relaxed)),
                    j.started.elapsed().as_secs_f32()
                ));
                let cur = p.current.lock().map(|c| c.clone()).unwrap_or_default();
                ui.add(egui::Label::new(RichText::new(cur).weak()).truncate());
            } else if let Some(bg) = &self.bg {
                ui.spinner();
                let mut t = format!("{}… {:.0} s", bg.label, bg.started.elapsed().as_secs_f32());
                if let Some(d) = &bg.dup {
                    let (a, b) = (d.hashed_bytes.load(Relaxed), d.total_bytes.load(Relaxed));
                    if b > 0 {
                        t += &format!(" — {} de {} lidos", fmt::size(a), fmt::size(b));
                    }
                }
                ui.label(t);
                if ui.small_button("Cancelar").clicked() {
                    bg.cancel.store(true, Relaxed);
                }
            } else if let Some(s) = &self.scan {
                ui.label(format!(
                    "{} — {} em {} arquivos, {} pastas; {} {}",
                    s.root_path.display(),
                    self.sz(s.root.size),
                    fmt::int(s.root.files as i64),
                    fmt::int(s.root.dirs as i64),
                    if s.folders_only { "instantâneo de" } else { "varrido em" },
                    fmt::date(s.when)
                ));
                if s.root.cloud > 0 {
                    ui.separator();
                    ui.label(RichText::new(format!("Nuvem: {} só online (não ocupa este disco)", self.sz(s.root.cloud))).color(Color32::from_rgb(0x8a, 0x9b, 0xe8)))
                        .on_hover_text("Arquivos sob demanda do OneDrive e similares. Veja em Exibir › Medida.");
                }
                if s.volume_total > 0 {
                    ui.separator();
                    ui.label(format!("Volume: {} livres de {}", self.sz(s.volume_free), self.sz(s.volume_total)));
                }
                if !s.errors.is_empty() && ui.link(format!("{} erros", s.errors.len())).clicked() {
                    self.tab = Tab::Errors;
                }
            } else {
                ui.label(if self.compact {
                    "Toque em Locais e escolha onde varrer."
                } else {
                    "Escolha uma unidade ou pasta e clique em Varrer. Também dá para arrastar uma pasta para a janela."
                });
            }
            if !self.status.is_empty() {
                ui.separator();
                ui.label(RichText::new(&self.status).color(ui.visuals().warn_fg_color));
            }
        });
    }

    fn windows(&mut self, ctx: &egui::Context, acts: &mut Vec<Act>) {
        if let Some((msg, paths)) = self.confirm.take() {
            let mut keep = true;
            egui::Window::new("Confirmar exclusão")
                .collapsible(false)
                .resizable(false)
                .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
                .show(ctx, |ui| {
                    ui.label(&msg);
                    ui.horizontal(|ui| {
                        if ui
                            .button(RichText::new("Excluir").color(Color32::from_rgb(220, 60, 60)))
                            .clicked()
                        {
                            acts.push(Act::Delete(paths.clone()));
                            keep = false;
                        }
                        if ui.button("Cancelar").clicked() {
                            keep = false;
                        }
                    });
                });
            if keep {
                self.confirm = Some((msg, paths));
            }
        }
        if let Some((paths, mut find, mut repl)) = self.rename.take() {
            let mut keep = true;
            egui::Window::new("Renomear vários").collapsible(false).show(ctx, |ui| {
                ui.label(format!("{} itens. Use expressão regular; $1, $2 inserem grupos.", paths.len()));
                egui::Grid::new("ren").num_columns(2).show(ui, |ui| {
                    ui.label("Procurar");
                    ui.text_edit_singleline(&mut find);
                    ui.end_row();
                    ui.label("Trocar por");
                    ui.text_edit_singleline(&mut repl);
                    ui.end_row();
                });
                match ops::rename(&paths, &find, &repl, true) {
                    Ok((plan, _)) if !find.is_empty() => {
                        ui.label(format!("{} nomes mudam. Exemplos:", plan.len()));
                        for (a, b) in plan.iter().take(6) {
                            ui.label(
                                RichText::new(format!(
                                    "{} → {}",
                                    a.file_name().unwrap_or_default().to_string_lossy(),
                                    b.file_name().unwrap_or_default().to_string_lossy()
                                ))
                                .monospace(),
                            );
                        }
                        if ui.add_enabled(!plan.is_empty(), egui::Button::new("Renomear")).clicked() {
                            match ops::rename(&paths, &find, &repl, false) {
                                Ok((_, rep)) => {
                                    self.status = format!("Renomeados: {}; falhas: {}. Atualize a varredura para ver os novos nomes.", rep.ok, rep.failed.len());
                                }
                                Err(e) => self.status = e,
                            }
                            keep = false;
                        }
                    }
                    Err(e) if !find.is_empty() => {
                        ui.colored_label(Color32::RED, e);
                    }
                    _ => {}
                }
                if ui.button("Fechar").clicked() {
                    keep = false;
                }
            });
            if keep {
                self.rename = Some((paths, find, repl));
            }
        }
        if self.show_options {
            let mut open = true;
            egui::Window::new("Opções de varredura")
                .open(&mut open)
                .show(ctx, |ui| {
                    let c = &mut self.cfg;
                    egui::Grid::new("opts")
                        .num_columns(2)
                        .spacing([12.0, 6.0])
                        .show(ui, |ui| {
                            ui.label("Threads");
                            ui.add(egui::DragValue::new(&mut c.threads).range(1..=64));
                            ui.end_row();
                            ui.label("Excluir (curingas ; )");
                            ui.text_edit_singleline(&mut c.exclude);
                            ui.end_row();
                            ui.label("Só arquivos (curingas ; )");
                            ui.text_edit_singleline(&mut c.include);
                            ui.end_row();
                            ui.label("");
                            ui.checkbox(&mut c.regex, "Os filtros são expressões regulares");
                            ui.end_row();
                        });
                    ui.checkbox(
                        &mut c.follow_links,
                        "Seguir links simbólicos e junções (pode contar em dobro)",
                    );
                    ui.checkbox(
                        &mut c.dedup_hardlinks,
                        "Contar hardlinks uma única vez (mais lento)",
                    );
                    ui.checkbox(&mut c.streams, "Somar fluxos alternativos NTFS (ADS)");
                    ui.checkbox(
                        &mut c.skip_cloud,
                        "Ignorar arquivos que estão só na nuvem (OneDrive etc.)",
                    );
                    ui.checkbox(&mut c.history, "Gravar no histórico após cada varredura");
                    if let Err(e) = c.scan_options() {
                        ui.colored_label(Color32::RED, format!("Filtro inválido: {e}"));
                    }
                    ui.label(
                        RichText::new(format!("Dados: {}", snapshot::data_dir().display())).weak(),
                    );
                });
            self.show_options = open;
        }
        if self.show_schedule {
            let mut open = true;
            egui::Window::new("Agendar varredura").open(&mut open).show(ctx, |ui| {
                let s = &mut self.sched;
                const FREQ: [&str; 3] = ["Diária", "Semanal", "Mensal"];
                egui::Grid::new("ag").num_columns(2).spacing([12.0, 6.0]).show(ui, |ui| {
                    ui.label("Pasta");
                    ui.text_edit_singleline(&mut s.path);
                    ui.end_row();
                    ui.label("Frequência");
                    egui::ComboBox::from_id_salt("freq").selected_text(FREQ[s.freq]).show_ui(ui, |ui| {
                        for (i, l) in FREQ.iter().enumerate() {
                            ui.selectable_value(&mut s.freq, i, *l);
                        }
                    });
                    ui.end_row();
                    ui.label("Horário (HH:MM)");
                    ui.text_edit_singleline(&mut s.time);
                    ui.end_row();
                    ui.label("Pasta dos relatórios");
                    ui.horizontal(|ui| {
                        ui.text_edit_singleline(&mut s.out_dir);
                        if ui.button("…").clicked() {
                            if let Some(p) = rfd::FileDialog::new().pick_folder() {
                                s.out_dir = p.to_string_lossy().into_owned();
                            }
                        }
                    });
                    ui.end_row();
                    ui.label("");
                    ui.checkbox(&mut s.xlsx, "Relatório Excel (.xlsx)");
                    ui.end_row();
                    ui.label("");
                    ui.checkbox(&mut s.history, "Gravar no histórico");
                    ui.end_row();
                });
                if ui.button("Criar tarefa").clicked() {
                    self.status = config::schedule(s);
                }
                ui.label(RichText::new("Usa o atlasdisco-cli (mesma pasta do programa). No Windows cria a tarefa em \"AtlasDisco\\\" no Agendador.").weak());
            });
            self.show_schedule = open;
        }
        if self.show_about {
            let mut open = true;
            egui::Window::new("Sobre")
                .open(&mut open)
                .collapsible(false)
                .show(ctx, |ui| {
                    ui.heading("AtlasDisco");
                    ui.label(format!("Versão {}", env!("CARGO_PKG_VERSION")));
                    ui.label("Analisador de espaço em disco multiplataforma, escrito em Rust.");
                });
            self.show_about = open;
        }
    }
}

fn retain_marked(hits: &mut Vec<Hit>, marked: &mut Vec<bool>, keep: &[bool]) {
    let mut i = 0;
    hits.retain(|_| {
        i += 1;
        keep[i - 1]
    });
    let mut j = 0;
    marked.retain(|_| {
        j += 1;
        keep.get(j - 1).copied().unwrap_or(false)
    });
}

impl eframe::App for App {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.poll(ctx);
        // Android 15+: o app ocupa a tela inteira; reserva o espaço das barras do sistema e do teclado.
        #[cfg(target_os = "android")]
        android::insets(ctx);
        let mut acts = Vec::new();
        // Arrastar uma pasta para a janela inicia a varredura.
        let dropped: Vec<PathBuf> = ctx.input(|i| {
            i.raw
                .dropped_files
                .iter()
                .filter_map(|f| f.path.clone())
                .collect()
        });
        if let Some(p) = dropped.into_iter().next() {
            acts.push(Act::Scan(if p.is_dir() {
                p
            } else {
                p.parent().map(Path::to_path_buf).unwrap_or(p)
            }));
        }
        egui::TopBottomPanel::top("topo").show(ctx, |ui| {
            self.menus(ui, ctx, &mut acts);
            self.toolbar(ui, &mut acts);
            ui.add_space(2.0);
        });
        egui::TopBottomPanel::bottom("status").show(ctx, |ui| self.status_bar(ui));
        let scan = self.scan.clone();
        // Tela estreita (celular): a árvore vira a aba "Pastas" e os controles ficam maiores.
        let compact = ctx.screen_rect().width() < 720.0;
        if compact != self.compact {
            self.compact = compact;
            ctx.style_mut(|s| {
                s.spacing.interact_size.y = if compact { 34.0 } else { 20.0 };
                s.spacing.button_padding = if compact {
                    egui::vec2(10.0, 6.0)
                } else {
                    egui::vec2(4.0, 1.0)
                };
                s.spacing.item_spacing = if compact {
                    egui::vec2(10.0, 8.0)
                } else {
                    egui::vec2(8.0, 3.0)
                };
            });
            if compact && self.tab == Tab::Details && self.sel.is_empty() {
                self.tab = Tab::Folders;
            }
        }
        if !compact && self.tab == Tab::Folders {
            self.tab = Tab::Details;
        }
        #[cfg(target_os = "android")]
        android::frame(ctx, self, &mut acts);
        if !compact {
            egui::SidePanel::left("arvore")
                .resizable(true)
                .default_width(380.0)
                .width_range(200.0..=900.0)
                .show(ctx, |ui| {
                    if let Some(s) = scan.as_deref() {
                        views::tree(self, ui, s, &mut acts);
                    } else {
                        ui.centered_and_justified(|ui| {
                            ui.label(RichText::new("Nenhuma varredura").weak())
                        });
                    }
                });
        }
        egui::CentralPanel::default().show(ctx, |ui| {
            ui.horizontal_wrapped(|ui| {
                if compact {
                    ui.selectable_value(&mut self.tab, Tab::Folders, "Pastas");
                }
                for (t, l) in Tab::ALL {
                    ui.selectable_value(&mut self.tab, t, l);
                }
            });
            ui.separator();
            match scan.as_deref() {
                Some(s) => views::central(self, ui, s, &mut acts),
                None => {
                    ui.centered_and_justified(|ui| {
                        ui.label(RichText::new(if self.compact { "Toque em Locais e escolha o armazenamento para ver o que ocupa espaço." } else { "Escolha uma unidade, digite um caminho ou arraste uma pasta para cá." }).size(16.0).weak());
                    });
                }
            }
        });
        self.windows(ctx, &mut acts);
        self.apply(ctx, acts);
        self.expand_to_sel = false;

        // Captura de tela automática (testes/documentação): ATLASDISCO_SCREENSHOT=arquivo.png
        if let Some((path, frames)) = self.screenshot.as_mut() {
            if self.job.is_none() && self.scan.is_some() {
                *frames += 1;
                if *frames == 8 {
                    ctx.send_viewport_cmd(egui::ViewportCommand::Screenshot(Default::default()));
                }
                let shot = ctx.input(|i| {
                    i.raw.events.iter().find_map(|e| {
                        if let egui::Event::Screenshot { image, .. } = e {
                            Some(image.clone())
                        } else {
                            None
                        }
                    })
                });
                if let Some(img) = shot {
                    config::save_png(path, &img);
                    ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                }
                ctx.request_repaint();
            }
        }
    }

    fn on_exit(&mut self, _gl: Option<&eframe::glow::Context>) {
        self.cfg.save();
    }
}
