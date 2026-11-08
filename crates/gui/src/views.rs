//! Árvore de pastas e abas da área central.

#[cfg(target_os = "android")]
use crate::rfd;
use crate::treemap::{self, Measure};
use crate::{Act, App, Col, Tab};
use atlasdisco_core::analysis::{self, DateKind};
use atlasdisco_core::export::{self, Format};
use atlasdisco_core::search::{self, DupMode, Kind};
use atlasdisco_core::snapshot::{self, Status};
use atlasdisco_core::{fmt, platform, Node, Scan};
use eframe::egui::{self, Color32, RichText, Sense, Vec2};
use egui_extras::{Column, TableBuilder};
use std::path::{Path, PathBuf};
use std::sync::Arc;

const ROW: f32 = 20.0;

// ------------------------------------------------------------------ árvore

pub fn tree(app: &mut App, ui: &mut egui::Ui, scan: &Scan, acts: &mut Vec<Act>) {
    egui::ScrollArea::both()
        .auto_shrink([false; 2])
        .show(ui, |ui| {
            let mut idx = Vec::new();
            let total = app.val(&scan.root);
            tree_node(app, ui, scan, &scan.root, &mut idx, total, total, acts);
        });
}

#[allow(clippy::too_many_arguments)]
fn tree_node(
    app: &mut App,
    ui: &mut egui::Ui,
    scan: &Scan,
    node: &Node,
    idx: &mut Vec<usize>,
    parent: u64,
    total: u64,
    acts: &mut Vec<Act>,
) {
    let id = ui.make_persistent_id(("arvore", app.gen, idx.clone()));
    let mut state = egui::collapsing_header::CollapsingState::load_with_default_open(
        ui.ctx(),
        id,
        idx.is_empty(),
    );
    if app.expand_to_sel && app.sel.len() > idx.len() && app.sel.starts_with(idx) {
        state.set_open(true);
    }
    let header = state.show_header(ui, |ui| {
        tree_label(app, ui, scan, node, idx, parent, total, acts)
    });
    header.body(|ui| {
        let order = sorted_children(app, node);
        let shown = order.len().min(500);
        for &i in &order[..shown] {
            let c = &node.children[i];
            idx.push(i);
            if c.is_dir && !c.link {
                tree_node(app, ui, scan, c, idx, app.val(node), total, acts);
            } else {
                ui.horizontal(|ui| {
                    ui.add_space(18.0);
                    tree_label(app, ui, scan, c, idx, app.val(node), total, acts);
                });
            }
            idx.pop();
        }
        if order.len() > shown {
            ui.label(
                RichText::new(format!(
                    "… e mais {} itens (veja em Detalhes)",
                    order.len() - shown
                ))
                .weak(),
            );
        }
    });
}

fn sorted_children(app: &App, node: &Node) -> Vec<usize> {
    let mut v: Vec<usize> = node
        .children
        .iter()
        .enumerate()
        .filter(|(_, c)| {
            (c.is_dir || app.cfg.files_in_tree) && !(app.cfg.hide_empty && c.is_dir && c.files == 0)
        })
        .map(|(i, _)| i)
        .collect();
    if app.measure != Measure::Size {
        v.sort_by(|&a, &b| app.val(&node.children[b]).cmp(&app.val(&node.children[a])));
    }
    v
}

#[allow(clippy::too_many_arguments)]
fn tree_label(
    app: &mut App,
    ui: &mut egui::Ui,
    scan: &Scan,
    n: &Node,
    idx: &[usize],
    parent: u64,
    total: u64,
    acts: &mut Vec<Act>,
) {
    let v = app.val(n);
    let frac = if parent > 0 {
        v as f32 / parent as f32
    } else {
        1.0
    };
    let (r, _) = ui.allocate_exact_size(Vec2::new(46.0, 12.0), Sense::hover());
    ui.painter()
        .rect_filled(r, 2.0, ui.visuals().faint_bg_color);
    let fill = egui::Rect::from_min_size(
        r.min,
        Vec2::new(r.width() * frac.clamp(0.0, 1.0), r.height()),
    );
    let col = if n.is_dir {
        Color32::from_rgb(0x3f, 0x8f, 0xe6)
    } else {
        treemap::file_color(&n.ext())
    };
    ui.painter().rect_filled(fill, 2.0, col);
    let name = if idx.is_empty() {
        scan.root_path.to_string_lossy().into_owned()
    } else {
        n.name.clone()
    };
    let mut text = RichText::new(format!(
        "{}   {}  ·  {}",
        name,
        app.val_str(v),
        fmt::pct(frac as f64)
    ));
    // Itens com 15 % ou mais do total aparecem em negrito.
    if total > 0 && v * 100 / total.max(1) >= 15 && !idx.is_empty() {
        text = text.strong();
    }
    if n.attrs & atlasdisco_core::model::ATTR_COMPRESSED != 0 {
        text = text.color(Color32::from_rgb(0x4a, 0x90, 0xe2));
    }
    if n.error.is_some() {
        text = text.color(Color32::from_rgb(0xd9, 0x4f, 0x4f));
    }
    if n.link {
        text = text.italics();
    }
    // Tudo (ou o arquivo) só na nuvem: azul-lavanda e itálico.
    if n.size > 0 && n.cloud == n.size {
        text = text.italics().color(Color32::from_rgb(0x8a, 0x9b, 0xe8));
    }
    // Item informativo (ex.: espaço dos apps no Android): âmbar, sem ações.
    if n.synthetic {
        text = text.color(Color32::from_rgb(0xe0, 0xa8, 0x3a));
    }
    let resp = ui.selectable_label(app.sel.as_slice() == idx, text);
    let resp = resp.on_hover_text(hover_info(app, n));
    if resp.clicked() {
        if n.is_dir {
            acts.push(Act::Select(idx.to_vec()));
        } else {
            acts.push(Act::Select(idx[..idx.len() - 1].to_vec()));
        }
    }
    let path = scan.path_of(idx);
    if !n.synthetic {
        resp.context_menu(|ui| item_menu(ui, &path, n.is_dir, Some(idx.to_vec()), acts));
    }
}

fn hover_info(app: &App, n: &Node) -> String {
    let mut s = format!("Tamanho: {}\nAlocado: {}", app.sz(n.size), app.sz(n.alloc));
    if n.synthetic {
        s += "\nInformativo: espaço contado pelo sistema, que nenhum app pode abrir ou apagar daqui.";
    }
    if n.cloud > 0 {
        s += &format!("\nSó na nuvem: {} (não ocupa este disco)", app.sz(n.cloud));
    }
    if n.is_dir {
        s += &format!(
            "\nArquivos: {}\nPastas: {}",
            fmt::int(n.files as i64),
            fmt::int(n.dirs as i64)
        );
    }
    s += &format!("\nModificado: {}", fmt::date(n.mtime));
    if n.link {
        s += "\nLink simbólico/junção (não seguido)";
    }
    if let Some(e) = &n.error {
        s += &format!("\nErro: {e}");
    }
    s
}

/// Menu de contexto comum a todas as listas.
pub fn item_menu(
    ui: &mut egui::Ui,
    path: &Path,
    is_dir: bool,
    idx: Option<Vec<usize>>,
    acts: &mut Vec<Act>,
) {
    menu_for(ui, &[path.to_path_buf()], is_dir, idx, acts);
}

fn menu_for(
    ui: &mut egui::Ui,
    paths: &[PathBuf],
    is_dir: bool,
    idx: Option<Vec<usize>>,
    acts: &mut Vec<Act>,
) {
    let single = paths.len() == 1;
    let p = &paths[0];
    let mut push = |ui: &mut egui::Ui, a: Act| {
        acts.push(a);
        ui.close_menu();
    };
    if single {
        if ui.button("Abrir").clicked() {
            push(ui, Act::Open(p.clone()));
        }
        if ui.button("Mostrar no gerenciador de arquivos").clicked() {
            push(ui, Act::Reveal(p.clone()));
        }
        if ui.button("Copiar caminho").clicked() {
            push(ui, Act::Copy(p.to_string_lossy().into_owned()));
        }
        if is_dir {
            if let Some(i) = idx {
                if ui.button("Selecionar na árvore").clicked() {
                    push(ui, Act::Select(i));
                }
            }
            if ui.button("Varrer só esta pasta").clicked() {
                push(ui, Act::Scan(p.clone()));
            }
        }
        ui.separator();
    } else {
        ui.label(RichText::new(format!("{} itens selecionados", paths.len())).weak());
        if ui.button("Copiar caminhos").clicked() {
            push(
                ui,
                Act::Copy(
                    paths
                        .iter()
                        .map(|p| p.to_string_lossy())
                        .collect::<Vec<_>>()
                        .join("\n"),
                ),
            );
        }
        ui.separator();
    }
    if ui.button("Copiar para…").clicked() {
        push(ui, Act::Transfer(paths.to_vec(), false));
    }
    if ui.button("Mover para…").clicked() {
        push(ui, Act::Transfer(paths.to_vec(), true));
    }
    if ui.button("Renomear…").clicked() {
        push(ui, Act::Rename(paths.to_vec()));
    }
    ui.separator();
    if ui.button("🗑 Enviar para a lixeira").clicked() {
        push(ui, Act::Trash(paths.to_vec()));
    }
    if ui
        .button(RichText::new("Excluir definitivamente…").color(Color32::from_rgb(220, 70, 70)))
        .clicked()
    {
        push(ui, Act::AskDelete(paths.to_vec()));
    }
    if cfg!(windows) && single {
        ui.separator();
        if ui.button("Compactar (NTFS)").clicked() {
            push(ui, Act::Compress(p.clone(), true));
        }
        if ui.button("Descompactar (NTFS)").clicked() {
            push(ui, Act::Compress(p.clone(), false));
        }
    }
}

// ------------------------------------------------------------------ área central

pub fn central(app: &mut App, ui: &mut egui::Ui, scan: &Scan, acts: &mut Vec<Act>) {
    let node = scan.root.get(&app.sel).unwrap_or(&scan.root);
    breadcrumb(app, ui, scan, acts);
    let key = (app.gen, app.sel.clone());
    if app.caches.key.as_ref() != Some(&key) {
        app.caches.key = Some(key);
        app.caches.ext = None;
        app.caches.cats = None;
        app.caches.ages = None;
        app.caches.top = None;
    }
    let base = scan.path_of(&app.sel);
    match app.tab {
        Tab::Folders => tree(app, ui, scan, acts),
        Tab::Details => details(app, ui, scan, node, acts),
        Tab::Treemap => treemap_tab(app, ui, scan, node, acts),
        Tab::Chart => chart_tab(app, ui, node),
        Tab::Extensions => ext_tab(app, ui, node, &base),
        Tab::Age => age_tab(app, ui, node, &base),
        Tab::Largest => largest_tab(app, ui, node, &base, acts),
        Tab::Owners => owners_tab(app, ui, node),
        Tab::Search => search_tab(app, ui, &base, acts),
        Tab::Duplicates => dup_tab(app, ui, &base, acts),
        Tab::Compare => compare_tab(app, ui, scan),
        Tab::History => history_tab(app, ui, scan),
        Tab::Errors => errors_tab(ui, scan),
    }
}

fn breadcrumb(app: &App, ui: &mut egui::Ui, scan: &Scan, acts: &mut Vec<Act>) {
    ui.horizontal_wrapped(|ui| {
        if ui.link(scan.root_path.to_string_lossy()).clicked() {
            acts.push(Act::Select(vec![]));
        }
        let mut n = &scan.root;
        for (k, &i) in app.sel.iter().enumerate() {
            n = &n.children[i];
            ui.label("›");
            if ui.link(&n.name).clicked() {
                acts.push(Act::Select(app.sel[..=k].to_vec()));
            }
        }
        let node = scan.root.get(&app.sel).unwrap_or(&scan.root);
        ui.label(
            RichText::new(format!(
                "  {} · {} arquivos · {} pastas",
                app.val_str(app.val(node)),
                fmt::int(node.files as i64),
                fmt::int(node.dirs as i64)
            ))
            .weak(),
        );
    });
    ui.add_space(4.0);
}

fn cell_text(
    app: &App,
    col: Col,
    c: &Node,
    parent: u64,
    path: &Path,
    owners: &mut std::collections::HashMap<PathBuf, String>,
) -> (String, bool) {
    let num = true;
    match col {
        Col::Name => (
            if c.is_dir {
                format!("📁 {}", c.name)
            } else {
                c.name.clone()
            },
            false,
        ),
        Col::Size => (app.sz(c.size), num),
        Col::Alloc => (app.sz(c.alloc), num),
        Col::Pct => (
            fmt::pct(if parent > 0 {
                app.val(c) as f64 / parent as f64
            } else {
                0.0
            }),
            num,
        ),
        Col::Files => (
            if c.is_dir {
                fmt::int(c.files as i64)
            } else {
                String::new()
            },
            num,
        ),
        Col::Dirs => (
            if c.is_dir {
                fmt::int(c.dirs as i64)
            } else {
                String::new()
            },
            num,
        ),
        Col::Modified => (fmt::date(c.mtime), false),
        Col::Accessed => (fmt::date(c.atime), false),
        Col::Created => (fmt::date(c.ctime), false),
        Col::Attrs => (fmt::attrs(c.attrs), false),
        Col::Ext => (
            if c.is_dir {
                if c.link {
                    "Link".into()
                } else {
                    "Pasta".into()
                }
            } else {
                format!("{} ({})", analysis::category(&c.ext()), c.ext())
            },
            false,
        ),
        Col::Compression => (
            if c.compression() > 0.0 {
                fmt::pct(c.compression())
            } else {
                String::new()
            },
            num,
        ),
        Col::Avg => (
            if c.is_dir && c.files > 0 {
                app.sz(c.size / c.files)
            } else {
                String::new()
            },
            num,
        ),
        Col::PathLen => (fmt::int(path.to_string_lossy().chars().count() as i64), num),
        Col::Cloud => (
            if c.cloud > 0 {
                app.sz(c.cloud)
            } else {
                String::new()
            },
            num,
        ),
        Col::Owner => (
            owners
                .entry(path.to_path_buf())
                .or_insert_with(|| platform::owner(path).unwrap_or_default())
                .clone(),
            false,
        ),
    }
}

fn sort_key_cmp(col: Col, a: &Node, b: &Node, app: &App) -> std::cmp::Ordering {
    match col {
        Col::Name => a.name.to_lowercase().cmp(&b.name.to_lowercase()),
        Col::Size => a.size.cmp(&b.size),
        Col::Alloc => a.alloc.cmp(&b.alloc),
        Col::Pct => app.val(a).cmp(&app.val(b)),
        Col::Files => a.files.cmp(&b.files),
        Col::Dirs => a.dirs.cmp(&b.dirs),
        Col::Modified => a.mtime.cmp(&b.mtime),
        Col::Accessed => a.atime.cmp(&b.atime),
        Col::Created => a.ctime.cmp(&b.ctime),
        Col::Attrs => a.attrs.cmp(&b.attrs),
        Col::Ext => a.ext().cmp(&b.ext()),
        Col::Compression => a.compression().total_cmp(&b.compression()),
        Col::Avg => (a.size / a.files.max(1)).cmp(&(b.size / b.files.max(1))),
        Col::PathLen => a.name.len().cmp(&b.name.len()),
        Col::Owner => std::cmp::Ordering::Equal,
        Col::Cloud => a.cloud.cmp(&b.cloud),
    }
}

fn details(app: &mut App, ui: &mut egui::Ui, scan: &Scan, node: &Node, acts: &mut Vec<Act>) {
    let key = (
        app.gen,
        app.sel.clone(),
        app.sort_col,
        app.sort_asc,
        app.filter.to_lowercase(),
        app.measure,
        app.cfg.hide_empty,
    );
    if app.caches.details.as_ref().map(|d| &d.0) != Some(&key) {
        let f = app.filter.to_lowercase();
        let col = Col::ALL[app.sort_col].0;
        let mut v: Vec<usize> = (0..node.children.len())
            .filter(|&i| {
                let c = &node.children[i];
                (f.is_empty() || c.name.to_lowercase().contains(&f))
                    && !(app.cfg.hide_empty && c.is_dir && c.files == 0)
            })
            .collect();
        v.sort_by(|&a, &b| {
            let o = sort_key_cmp(col, &node.children[a], &node.children[b], app);
            if app.sort_asc {
                o
            } else {
                o.reverse()
            }
        });
        app.caches.details = Some((key, v));
    }
    let order = app.caches.details.as_ref().unwrap().1.clone();
    let cols: Vec<usize> = (0..Col::ALL.len()).filter(|&k| app.cfg.cols[k]).collect();
    let parent = app.val(node);
    let base = scan.path_of(&app.sel);
    let selected: Vec<PathBuf> = app
        .detail_sel
        .iter()
        .filter_map(|&i| node.children.get(i))
        .map(|c| base.join(&c.name))
        .collect();

    ui.horizontal(|ui| {
        ui.label(RichText::new(format!("{} itens", fmt::int(order.len() as i64))).weak());
        if !app.detail_sel.is_empty() {
            ui.label(format!("· {} selecionados", app.detail_sel.len()));
            if ui.small_button("Limpar seleção").clicked() {
                app.detail_sel.clear();
            }
        }
        ui.label(
            RichText::new(if app.compact {
                "Toque duplo abre · toque longo: ações"
            } else {
                "Ctrl+clique seleciona vários · duplo clique abre a pasta · botão direito: ações"
            })
            .weak(),
        );
    });

    let mut owners = std::mem::take(&mut app.caches.owners_of);
    // Em tela estreita, a tabela rola na horizontal.
    egui::ScrollArea::horizontal()
        .id_salt("detalhes-h")
        .show(ui, |ui| {
            let mut table = TableBuilder::new(ui)
                .striped(true)
                .resizable(true)
                .cell_layout(egui::Layout::left_to_right(egui::Align::Center));
            for (k, &c) in cols.iter().enumerate() {
                table = table.column(if k == 0 {
                    Column::initial(320.0).at_least(120.0).clip(true)
                } else {
                    Column::initial(if c == 6 || c == 7 || c == 8 {
                        120.0
                    } else {
                        90.0
                    })
                    .at_least(40.0)
                    .clip(true)
                });
            }
            table
                .header(ROW + 2.0, |mut h| {
                    for &c in &cols {
                        h.col(|ui| {
                            let mut label = Col::ALL[c].1.to_string();
                            if app.sort_col == c {
                                label += if app.sort_asc { " ⬆" } else { " ⬇" };
                            }
                            if ui
                                .add(egui::Button::new(RichText::new(label).strong()).frame(false))
                                .clicked()
                            {
                                if app.sort_col == c {
                                    app.sort_asc = !app.sort_asc;
                                } else {
                                    app.sort_col = c;
                                    app.sort_asc = c == 0;
                                }
                            }
                        });
                    }
                })
                .body(|body| {
                    body.rows(ROW, order.len(), |mut row| {
                        let i = order[row.index()];
                        let c = &node.children[i];
                        let path = base.join(&c.name);
                        let is_sel = app.detail_sel.contains(&i);
                        row.set_selected(is_sel);
                        for (k, &col) in cols.iter().enumerate() {
                            let (text, num) =
                                cell_text(app, Col::ALL[col].0, c, parent, &path, &mut owners);
                            row.col(|ui| {
                                if k == 0 {
                                    let mut rt = RichText::new(text);
                                    if c.attrs & atlasdisco_core::model::ATTR_COMPRESSED != 0 {
                                        rt = rt.color(Color32::from_rgb(0x4a, 0x90, 0xe2));
                                    }
                                    let r = ui
                                        .add(egui::Label::new(rt).sense(Sense::click()).truncate());
                                    if r.clicked() {
                                        if ui.input(|i| i.modifiers.command) {
                                            if !app.detail_sel.remove(&i) {
                                                app.detail_sel.insert(i);
                                            }
                                        } else {
                                            app.detail_sel.clear();
                                            app.detail_sel.insert(i);
                                        }
                                    }
                                    if r.double_clicked() {
                                        if c.is_dir && !c.link {
                                            let mut s = app.sel.clone();
                                            s.push(i);
                                            acts.push(Act::Select(s));
                                        } else if !c.synthetic {
                                            acts.push(Act::Open(path.clone()));
                                        }
                                    }
                                    if !c.synthetic {
                                        r.context_menu(|ui| {
                                            if is_sel && selected.len() > 1 {
                                                menu_for(ui, &selected, false, None, acts);
                                            } else {
                                                let mut s = app.sel.clone();
                                                s.push(i);
                                                item_menu(ui, &path, c.is_dir, Some(s), acts);
                                            }
                                        });
                                    }
                                } else if num {
                                    ui.with_layout(
                                        egui::Layout::right_to_left(egui::Align::Center),
                                        |ui| ui.label(text),
                                    );
                                } else {
                                    ui.label(text);
                                }
                            });
                        }
                    });
                });
        });
    app.caches.owners_of = owners;
}

fn treemap_tab(app: &mut App, ui: &mut egui::Ui, scan: &Scan, node: &Node, acts: &mut Vec<Act>) {
    ui.horizontal_wrapped(|ui| {
        ui.label("Nível de detalhe");
        ui.add(egui::Slider::new(&mut app.cfg.treemap_depth, 1..=12));
        ui.checkbox(&mut app.tm_files, "Arquivos individuais");
        ui.checkbox(&mut app.tm_hierarchy, "Hierarquia");
        ui.label(
            RichText::new(if app.compact {
                "Toque duplo entra na pasta · toque longo: ações"
            } else {
                "Clique: destacar · duplo clique: entrar na pasta · botão direito: ações"
            })
            .weak(),
        );
    });
    let (rect, resp) = ui.allocate_exact_size(ui.available_size(), Sense::click());
    let key = (
        app.gen,
        app.sel.clone(),
        [
            rect.min.x as i32,
            rect.min.y as i32,
            rect.max.x as i32,
            rect.max.y as i32,
        ],
        app.cfg.treemap_depth,
        app.measure,
        app.tm_files,
        app.tm_hierarchy,
    );
    if app.caches.tiles.as_ref().map(|t| &t.0) != Some(&key) {
        let p = treemap::Params {
            max_depth: app.cfg.treemap_depth,
            measure: app.measure,
            files: app.tm_files,
            hierarchy: app.tm_hierarchy,
        };
        let tiles = treemap::layout(node, &app.sel, rect, &p);
        app.caches.tiles = Some((key, tiles));
    }
    let tiles = &app.caches.tiles.as_ref().unwrap().1;
    let painter = ui.painter_at(rect);
    painter.rect_filled(rect, 0.0, ui.visuals().extreme_bg_color);
    let hl_id = ui.id().with("destaque");
    let hl: Option<Vec<usize>> = ui.data(|d| d.get_temp(hl_id));
    treemap::paint(&painter, tiles, node, hl.as_deref(), app.sel.len());
    if let Some(pos) = resp.hover_pos() {
        if let Some(t) = treemap::hit(tiles, pos) {
            let n = scan.root.get(&t.idx);
            let path = scan.path_of(&t.idx);
            let info = match n {
                Some(n) if t.idx.len() > app.sel.len() => {
                    format!("{}\n{}", path.display(), hover_info(app, n))
                }
                _ => "Arquivos desta pasta (agrupados)".into(),
            };
            painter.rect_stroke(
                t.rect,
                0.0,
                egui::Stroke::new(1.5f32, Color32::WHITE),
                egui::StrokeKind::Inside,
            );
            let r2 = resp.clone().on_hover_text_at_pointer(info);
            if r2.clicked() {
                ui.data_mut(|d| d.insert_temp(hl_id, t.idx.clone()));
            }
            if r2.double_clicked() {
                if let Some(n) = n {
                    if n.is_dir {
                        acts.push(Act::Select(t.idx.clone()));
                    } else {
                        acts.push(Act::Open(path.clone()));
                    }
                }
            }
            let (is_dir, idx) = (n.map(|n| n.is_dir).unwrap_or(true), t.idx.clone());
            if !n.is_some_and(|n| n.synthetic) {
                resp.context_menu(|ui| item_menu(ui, &path, is_dir, Some(idx), acts));
            }
        }
    }
}

fn top_items(app: &App, node: &Node) -> Vec<(String, u64, Color32)> {
    let mut kids: Vec<&Node> = node.children.iter().collect();
    kids.sort_by_key(|c| std::cmp::Reverse(app.val(c)));
    let mut items: Vec<(String, u64, Color32)> = kids
        .iter()
        .take(11)
        .enumerate()
        .map(|(k, c)| (c.name.clone(), app.val(c), treemap::PALETTE[k]))
        .collect();
    let rest: u64 = kids.iter().skip(11).map(|c| app.val(c)).sum();
    if rest > 0 {
        items.push((
            format!("Outros ({} itens)", kids.len() - 11),
            rest,
            treemap::PALETTE[11],
        ));
    }
    items.retain(|x| x.1 > 0);
    items
}

fn chart_tab(app: &mut App, ui: &mut egui::Ui, node: &Node) {
    ui.horizontal(|ui| {
        ui.selectable_value(&mut app.chart_pie, true, "Pizza");
        ui.selectable_value(&mut app.chart_pie, false, "Barras");
    });
    let items = top_items(app, node);
    let f = |v: u64| app.val_str(v);
    egui::ScrollArea::vertical()
        .auto_shrink([false; 2])
        .show(ui, |ui| {
            if items.is_empty() {
                ui.label("Pasta vazia.");
            } else if app.chart_pie {
                treemap::pie(ui, &items, &f);
            } else {
                treemap::bars(ui, &items, &f);
            }
        });
}

fn ext_tab(app: &mut App, ui: &mut egui::Ui, node: &Node, base: &Path) {
    if app.caches.ext.is_none() {
        app.caches.ext = Some(analysis::extensions(node));
        app.caches.cats = Some(analysis::categories(node));
    }
    let ext = app.caches.ext.clone().unwrap();
    let cats = app.caches.cats.clone().unwrap();
    let total: u64 = ext.iter().map(|e| e.size).sum();
    ui.horizontal(|ui| {
        ui.label(RichText::new(format!("{} tipos de arquivo", ext.len())).weak());
        export_menu(app, ui, "ext", || {
            let mut t = export::Table {
                title: format!("Tipos de arquivo em {}", base.display()),
                headers: [
                    "Extensão",
                    "Categoria",
                    "Tamanho",
                    "Alocado",
                    "%",
                    "Arquivos",
                ]
                .map(String::from)
                .to_vec(),
                ..Default::default()
            };
            for e in &ext {
                t.rows.push(vec![
                    export::Cell::Text(e.ext.clone()),
                    export::Cell::Text(analysis::category(&e.ext).into()),
                    export::Cell::Bytes(e.size),
                    export::Cell::Bytes(e.alloc),
                    export::Cell::Pct(e.size as f64 / total.max(1) as f64),
                    export::Cell::Int(e.files as i64),
                ]);
            }
            t
        });
    });
    let items: Vec<(String, u64, Color32)> = cats
        .iter()
        .map(|c| (c.ext.clone(), c.size, treemap::category_color(&c.ext)))
        .collect();
    let f = |v: u64| app.sz(v);
    egui::CollapsingHeader::new("Por categoria")
        .default_open(true)
        .show(ui, |ui| treemap::bars(ui, &items, &f));
    ui.separator();
    ui.push_id("tabela-ext", |ui| {
        TableBuilder::new(ui)
            .striped(true)
            .resizable(true)
            .column(Column::initial(120.0))
            .column(Column::initial(130.0))
            .column(Column::initial(100.0))
            .column(Column::initial(70.0))
            .column(Column::initial(90.0))
            .column(Column::remainder())
            .header(ROW, |mut h| {
                for t in ["Extensão", "Categoria", "Tamanho", "%", "Arquivos", ""] {
                    h.col(|ui| {
                        ui.strong(t);
                    });
                }
            })
            .body(|body| {
                body.rows(ROW, ext.len(), |mut row| {
                    let e = &ext[row.index()];
                    let p = e.size as f64 / total.max(1) as f64;
                    row.col(|ui| {
                        ui.label(if e.ext.is_empty() {
                            "(sem extensão)".to_string()
                        } else {
                            format!(".{}", e.ext)
                        });
                    });
                    row.col(|ui| {
                        ui.label(analysis::category(&e.ext));
                    });
                    row.col(|ui| {
                        ui.label(app.sz(e.size));
                    });
                    row.col(|ui| {
                        ui.label(fmt::pct(p));
                    });
                    row.col(|ui| {
                        ui.label(fmt::int(e.files as i64));
                    });
                    row.col(|ui| bar(ui, p as f32, treemap::file_color(&e.ext)));
                });
            });
    });
}

fn bar(ui: &mut egui::Ui, frac: f32, col: Color32) {
    let w = ui.available_width().min(240.0);
    let (r, _) = ui.allocate_exact_size(Vec2::new(w, 10.0), Sense::hover());
    ui.painter()
        .rect_filled(r, 2.0, ui.visuals().faint_bg_color);
    ui.painter().rect_filled(
        egui::Rect::from_min_size(r.min, Vec2::new(w * frac.clamp(0.0, 1.0), 10.0)),
        2.0,
        col,
    );
}

fn export_menu(app: &mut App, ui: &mut egui::Ui, id: &str, make: impl FnOnce() -> export::Table) {
    let mut chosen = None;
    ui.push_id(id, |ui| {
        ui.menu_button("Exportar…", |ui| {
            for f in Format::ALL {
                if ui.button(f.name()).clicked() {
                    chosen = Some(f);
                    ui.close_menu();
                }
            }
        });
    });
    if let Some(f) = chosen {
        app.export_table(make(), f);
    }
}

fn age_tab(app: &mut App, ui: &mut egui::Ui, node: &Node, base: &Path) {
    ui.horizontal(|ui| {
        ui.label("Data considerada:");
        for (k, l) in [
            (DateKind::Modified, "Modificação"),
            (DateKind::Accessed, "Último acesso"),
            (DateKind::Created, "Criação"),
        ] {
            ui.selectable_value(&mut app.date_kind, k, l);
        }
    });
    if app.caches.ages.as_ref().map(|a| a.0) != Some(app.date_kind) {
        app.caches.ages = Some((
            app.date_kind,
            analysis::ages(node, app.date_kind, chrono::Utc::now().timestamp()),
        ));
    }
    let ages = app.caches.ages.clone().unwrap().1;
    let colors = [
        0x3b9e5f, 0x6bb26a, 0xa8c45a, 0xe0c03a, 0xe08e2b, 0xd9634f, 0xb03a3a, 0x888888,
    ];
    let items: Vec<(String, u64, Color32)> = ages
        .iter()
        .zip(colors)
        .map(|(a, c)| {
            (
                format!("{} ({} arquivos)", a.label, fmt::int(a.files as i64)),
                a.size,
                Color32::from_rgb((c >> 16) as u8, (c >> 8) as u8, c as u8),
            )
        })
        .collect();
    let f = |v: u64| app.sz(v);
    treemap::bars(ui, &items, &f);
    ui.add_space(8.0);
    export_menu(app, ui, "idade", || {
        let mut t = export::Table {
            title: format!("Idade dos arquivos em {}", base.display()),
            headers: ["Faixa", "Tamanho", "Arquivos"].map(String::from).to_vec(),
            ..Default::default()
        };
        for a in &ages {
            t.rows.push(vec![
                export::Cell::Text(a.label.into()),
                export::Cell::Bytes(a.size),
                export::Cell::Int(a.files as i64),
            ]);
        }
        t
    });
}

fn largest_tab(app: &mut App, ui: &mut egui::Ui, node: &Node, base: &Path, acts: &mut Vec<Act>) {
    if app.caches.top.is_none() {
        app.caches.top = Some(analysis::largest(node, base, 200));
    }
    let top = app.caches.top.clone().unwrap();
    let f = app.filter.to_lowercase();
    let rows: Vec<&analysis::FileRef> = top
        .iter()
        .filter(|r| f.is_empty() || r.path.to_string_lossy().to_lowercase().contains(&f))
        .collect();
    ui.horizontal(|ui| {
        ui.label(RichText::new(format!("Os {} maiores arquivos da seleção", rows.len())).weak());
        export_menu(app, ui, "maiores", || {
            let mut t = export::Table {
                title: format!("Maiores arquivos em {}", base.display()),
                headers: [
                    "Caminho",
                    "Tamanho",
                    "Alocado",
                    "Última modificação",
                    "Último acesso",
                ]
                .map(String::from)
                .to_vec(),
                ..Default::default()
            };
            for r in &top {
                t.rows.push(vec![
                    export::Cell::Text(r.path.to_string_lossy().into_owned()),
                    export::Cell::Bytes(r.size),
                    export::Cell::Bytes(r.alloc),
                    export::Cell::Date(r.mtime),
                    export::Cell::Date(r.atime),
                ]);
            }
            t
        });
    });
    ui.push_id("tabela-maiores", |ui| {
        TableBuilder::new(ui)
            .striped(true)
            .resizable(true)
            .column(Column::initial(100.0))
            .column(Column::initial(130.0))
            .column(Column::initial(130.0))
            .column(Column::remainder().clip(true))
            .header(ROW, |mut h| {
                for t in ["Tamanho", "Modificado", "Último acesso", "Caminho"] {
                    h.col(|ui| {
                        ui.strong(t);
                    });
                }
            })
            .body(|body| {
                body.rows(ROW, rows.len(), |mut row| {
                    let r = rows[row.index()];
                    row.col(|ui| {
                        ui.label(app.sz(r.size));
                    });
                    row.col(|ui| {
                        ui.label(fmt::date(r.mtime));
                    });
                    row.col(|ui| {
                        ui.label(fmt::date(r.atime));
                    });
                    row.col(|ui| {
                        let resp = ui.add(
                            egui::Label::new(r.path.to_string_lossy())
                                .sense(Sense::click())
                                .truncate(),
                        );
                        if resp.double_clicked() {
                            acts.push(Act::Reveal(r.path.clone()));
                        }
                        resp.context_menu(|ui| item_menu(ui, &r.path, false, None, acts));
                    });
                });
            });
    });
}

fn owners_tab(app: &mut App, ui: &mut egui::Ui, node: &Node) {
    ui.horizontal(|ui| {
        if ui.button("Calcular proprietários da seleção").clicked() {
            app.run_owners();
        }
        ui.label(
            RichText::new(format!(
                "Lê o dono de cada um dos {} arquivos; pode demorar em discos grandes.",
                fmt::int(node.files as i64)
            ))
            .weak(),
        );
    });
    if let Some(o) = app.owners.clone() {
        let items: Vec<(String, u64, Color32)> = o
            .iter()
            .enumerate()
            .map(|(k, x)| {
                (
                    format!("{} ({} arquivos)", x.owner, fmt::int(x.files as i64)),
                    x.size,
                    treemap::PALETTE[k % 12],
                )
            })
            .collect();
        let f = |v: u64| app.sz(v);
        egui::ScrollArea::vertical().show(ui, |ui| treemap::bars(ui, &items, &f));
    }
}

fn search_tab(app: &mut App, ui: &mut egui::Ui, base: &Path, acts: &mut Vec<Act>) {
    ui.label(RichText::new(format!("Busca dentro de: {}", base.display())).weak());
    ui.horizontal_wrapped(|ui| {
        ui.label("Atalhos:");
        let s = &mut app.search;
        let mut preset = |name: &str, f: &mut dyn FnMut(&mut crate::SearchUi)| {
            if ui.button(name).clicked() {
                s.name.clear();
                s.min.clear();
                s.max.clear();
                s.older.clear();
                s.newer.clear();
                s.pathlen.clear();
                s.crit = Default::default();
                f(s);
            }
        };
        preset("Arquivos grandes (≥ 100 MB)", &mut |s| {
            s.min = "100 MB".into()
        });
        preset("Antigos (sem mudar há 2 anos)", &mut |s| {
            s.older = "730".into()
        });
        preset("Temporários", &mut |s| {
            s.name = "*.tmp;*.temp;~*;*.bak;*.old;*.dmp;thumbs.db".into()
        });
        preset("Caminhos longos (≥ 260)", &mut |s| {
            s.pathlen = "260".into();
            s.crit.kind = Kind::Both
        });
        preset("Vídeos", &mut |s| {
            s.name = "*.mp4;*.mkv;*.avi;*.mov;*.wmv;*.webm".into()
        });
        preset("Pastas vazias", &mut |s| {
            s.crit.kind = Kind::Folders;
            s.max = "0".into()
        });
    });
    egui::Grid::new("busca")
        .num_columns(4)
        .spacing([10.0, 6.0])
        .show(ui, |ui| {
            let s = &mut app.search;
            ui.label("Nome");
            ui.add(egui::TextEdit::singleline(&mut s.name).hint_text("*.iso;backup*"));
            ui.checkbox(&mut s.crit.regex, "Expressão regular");
            egui::ComboBox::from_id_salt("tipo")
                .selected_text(match s.crit.kind {
                    Kind::Files => "Arquivos",
                    Kind::Folders => "Pastas",
                    Kind::Both => "Arquivos e pastas",
                })
                .show_ui(ui, |ui| {
                    ui.selectable_value(&mut s.crit.kind, Kind::Files, "Arquivos");
                    ui.selectable_value(&mut s.crit.kind, Kind::Folders, "Pastas");
                    ui.selectable_value(&mut s.crit.kind, Kind::Both, "Arquivos e pastas");
                });
            ui.end_row();
            ui.label("Tamanho mínimo");
            ui.add(egui::TextEdit::singleline(&mut s.min).hint_text("ex.: 50 MB"));
            ui.label("máximo");
            ui.add(egui::TextEdit::singleline(&mut s.max).hint_text("ex.: 2 GB"));
            ui.end_row();
            ui.label("Mais antigo que (dias)");
            ui.text_edit_singleline(&mut s.older);
            ui.label("mais novo que (dias)");
            ui.text_edit_singleline(&mut s.newer);
            ui.end_row();
            ui.label("Data considerada");
            egui::ComboBox::from_id_salt("data")
                .selected_text(
                    ["Modificação", "Último acesso", "Criação"]
                        [s.crit.date_kind_code.min(2) as usize],
                )
                .show_ui(ui, |ui| {
                    for (k, l) in ["Modificação", "Último acesso", "Criação"]
                        .iter()
                        .enumerate()
                    {
                        ui.selectable_value(&mut s.crit.date_kind_code, k as u8, *l);
                    }
                });
            ui.label("Caminho com ≥ caracteres");
            ui.text_edit_singleline(&mut s.pathlen);
            ui.end_row();
            ui.label("Caminho contém");
            ui.text_edit_singleline(&mut s.crit.path_contains);
            ui.label("Texto no conteúdo");
            ui.text_edit_singleline(&mut s.crit.content);
            ui.end_row();
        });
    ui.horizontal(|ui| {
        if ui.button(RichText::new("🔎 Buscar").strong()).clicked() {
            app.run_search();
        }
        if ui.button("Salvar modelo…").clicked() {
            if let Some(p) = rfd::FileDialog::new()
                .set_file_name("busca.json")
                .add_filter("Modelo de busca", &["json"])
                .save_file()
            {
                let mut c = app.search.crit.clone();
                c.name = app.search.name.clone();
                c.min_size = fmt::parse_size(&app.search.min);
                c.max_size = fmt::parse_size(&app.search.max);
                c.older_than_days = app.search.older.trim().parse().ok();
                c.newer_than_days = app.search.newer.trim().parse().ok();
                c.min_path_len = app.search.pathlen.trim().parse().ok();
                app.status = match std::fs::write(&p, c.to_json()) {
                    Ok(()) => format!("Modelo salvo: {}", p.display()),
                    Err(e) => e.to_string(),
                };
            }
        }
        if ui.button("Abrir modelo…").clicked() {
            if let Some(p) = rfd::FileDialog::new()
                .add_filter("Modelo de busca", &["json"])
                .pick_file()
            {
                match std::fs::read_to_string(&p)
                    .map_err(|e| e.to_string())
                    .and_then(|t| search::Criteria::from_json(&t))
                {
                    Ok(c) => {
                        let s = &mut app.search;
                        s.name = c.name.clone();
                        s.min = c.min_size.map(fmt::size).unwrap_or_default();
                        s.max = c.max_size.map(fmt::size).unwrap_or_default();
                        s.older = c.older_than_days.map(|v| v.to_string()).unwrap_or_default();
                        s.newer = c.newer_than_days.map(|v| v.to_string()).unwrap_or_default();
                        s.pathlen = c.min_path_len.map(|v| v.to_string()).unwrap_or_default();
                        s.crit = c;
                    }
                    Err(e) => app.status = e,
                }
            }
        }
    });
    ui.separator();
    let total: u64 = app.search.hits.iter().map(|h| h.size).sum();
    let hits = std::mem::take(&mut app.search.hits);
    let mut marked = std::mem::take(&mut app.search.marked);
    results_actions(
        app,
        ui,
        &hits,
        &mut marked,
        &format!(
            "{} resultados, {}",
            fmt::int(hits.len() as i64),
            app.sz(total)
        ),
        "busca",
        acts,
        false,
    );
    hits_table(app, ui, &hits, &mut marked, false, acts);
    app.search.hits = hits;
    app.search.marked = marked;
}

#[allow(clippy::too_many_arguments)]
fn results_actions(
    app: &mut App,
    ui: &mut egui::Ui,
    hits: &[search::Hit],
    marked: &mut [bool],
    summary: &str,
    id: &str,
    acts: &mut Vec<Act>,
    dups: bool,
) {
    let chosen: Vec<PathBuf> = hits
        .iter()
        .zip(marked.iter())
        .filter(|(_, m)| **m)
        .map(|(h, _)| h.path.clone())
        .collect();
    let chosen_size: u64 = hits
        .iter()
        .zip(marked.iter())
        .filter(|(_, m)| **m)
        .map(|(h, _)| h.size)
        .sum();
    ui.horizontal_wrapped(|ui| {
        ui.label(RichText::new(summary).strong());
        ui.separator();
        if ui.small_button("Marcar todos").clicked() {
            marked.iter_mut().for_each(|m| *m = true);
        }
        if ui.small_button("Nenhum").clicked() {
            marked.iter_mut().for_each(|m| *m = false);
        }
        if ui.small_button("Inverter").clicked() {
            marked.iter_mut().for_each(|m| *m = !*m);
        }
        ui.label(format!(
            "{} marcados ({})",
            chosen.len(),
            app.sz(chosen_size)
        ));
        ui.add_enabled_ui(!chosen.is_empty(), |ui| {
            if ui.button("🗑 Lixeira").clicked() {
                acts.push(Act::Trash(chosen.clone()));
            }
            if ui.button("Excluir…").clicked() {
                acts.push(Act::AskDelete(chosen.clone()));
            }
            if ui.button("Mover…").clicked() {
                acts.push(Act::Transfer(chosen.clone(), true));
            }
            if ui.button("Copiar…").clicked() {
                acts.push(Act::Transfer(chosen.clone(), false));
            }
            if ui.button("Renomear…").clicked() {
                acts.push(Act::Rename(chosen.clone()));
            }
        });
        let title = summary.to_string();
        export_menu(app, ui, id, || search::hits_table(title, hits, dups));
    });
}

fn hits_table(
    app: &App,
    ui: &mut egui::Ui,
    hits: &[search::Hit],
    marked: &mut [bool],
    dups: bool,
    acts: &mut Vec<Act>,
) {
    ui.push_id(("hits", dups), |ui| {
        let mut t = TableBuilder::new(ui)
            .striped(!dups)
            .resizable(true)
            .cell_layout(egui::Layout::left_to_right(egui::Align::Center))
            .column(Column::exact(24.0));
        if dups {
            t = t.column(Column::initial(50.0));
        }
        t.column(Column::initial(100.0))
            .column(Column::initial(130.0))
            .column(Column::remainder().clip(true))
            .header(ROW, |mut h| {
                h.col(|_| {});
                if dups {
                    h.col(|ui| {
                        ui.strong("Grupo");
                    });
                }
                for t in ["Tamanho", "Modificado", "Caminho"] {
                    h.col(|ui| {
                        ui.strong(t);
                    });
                }
            })
            .body(|body| {
                body.rows(ROW, hits.len(), |mut row| {
                    let i = row.index();
                    let h = &hits[i];
                    if dups && h.group.is_multiple_of(2) {
                        row.set_selected(true);
                    }
                    row.col(|ui| {
                        ui.checkbox(&mut marked[i], "");
                    });
                    if dups {
                        row.col(|ui| {
                            ui.label(h.group.to_string());
                        });
                    }
                    row.col(|ui| {
                        ui.label(app.sz(h.size));
                    });
                    row.col(|ui| {
                        ui.label(fmt::date(h.mtime));
                    });
                    row.col(|ui| {
                        let text = if h.is_dir {
                            format!("📁 {}", h.path.display())
                        } else {
                            h.path.display().to_string()
                        };
                        let r = ui.add(egui::Label::new(text).sense(Sense::click()).truncate());
                        if r.clicked() {
                            marked[i] = !marked[i];
                        }
                        if r.double_clicked() {
                            acts.push(Act::Reveal(h.path.clone()));
                        }
                        r.context_menu(|ui| item_menu(ui, &h.path, h.is_dir, None, acts));
                    });
                });
            });
    });
}

fn dup_tab(app: &mut App, ui: &mut egui::Ui, base: &Path, acts: &mut Vec<Act>) {
    ui.label(RichText::new(format!("Procura dentro de: {}", base.display())).weak());
    ui.horizontal(|ui| {
        ui.label("Tamanho mínimo");
        ui.add(egui::TextEdit::singleline(&mut app.dup.min).desired_width(80.0));
        ui.label("Comparar por");
        ui.selectable_value(&mut app.dup.mode, DupMode::Sha256, "Conteúdo (SHA-256)");
        ui.selectable_value(&mut app.dup.mode, DupMode::Md5, "Conteúdo (MD5)");
        ui.selectable_value(&mut app.dup.mode, DupMode::NameSize, "Nome e tamanho");
        if ui
            .button(RichText::new("Procurar duplicados").strong())
            .clicked()
        {
            app.run_dups();
        }
    });
    let hits = std::mem::take(&mut app.dup.hits);
    let mut marked = std::mem::take(&mut app.dup.marked);
    ui.horizontal(|ui| {
        if ui.button("Marcar cópias (manter o mais antigo de cada grupo)").clicked() {
            mark_copies(&hits, &mut marked);
        }
        let groups = hardlink_plan(&hits, &marked);
        if ui
            .add_enabled(!groups.is_empty() && app.dup.mode != DupMode::NameSize, egui::Button::new("Substituir marcados por hardlinks"))
            .on_hover_text("Cada marcado vira um link para o arquivo não marcado do grupo; o espaço é liberado e o arquivo continua no lugar. Só no mesmo volume.")
            .clicked()
        {
            acts.push(Act::Hardlink(groups));
        }
    });
    let summary = format!(
        "{} grupos · {} poderiam ser liberados",
        app.dup.groups,
        app.sz(app.dup.wasted)
    );
    results_actions(app, ui, &hits, &mut marked, &summary, "dups", acts, true);
    hits_table(app, ui, &hits, &mut marked, true, acts);
    app.dup.hits = hits;
    app.dup.marked = marked;
}

/// Em cada grupo, marca todos menos o arquivo mais antigo.
fn mark_copies(hits: &[search::Hit], marked: &mut [bool]) {
    let mut keep: std::collections::HashMap<usize, (i64, usize)> = Default::default();
    for (i, h) in hits.iter().enumerate() {
        let e = keep.entry(h.group).or_insert((h.mtime, i));
        if h.mtime < e.0 {
            *e = (h.mtime, i);
        }
    }
    for (i, h) in hits.iter().enumerate() {
        marked[i] = keep[&h.group].1 != i;
    }
}

fn hardlink_plan(hits: &[search::Hit], marked: &[bool]) -> Vec<(PathBuf, Vec<PathBuf>)> {
    let mut by: std::collections::BTreeMap<usize, (Option<PathBuf>, Vec<PathBuf>)> =
        Default::default();
    for (h, &m) in hits.iter().zip(marked) {
        let e = by.entry(h.group).or_default();
        if m {
            e.1.push(h.path.clone());
        } else if e.0.is_none() {
            e.0 = Some(h.path.clone());
        }
    }
    by.into_values()
        .filter_map(|(k, c)| Some((k?, c)))
        .filter(|(_, c)| !c.is_empty())
        .collect()
}

fn compare_tab(app: &mut App, ui: &mut egui::Ui, scan: &Scan) {
    ui.horizontal_wrapped(|ui| {
        ui.label("Comparar a varredura atual com:");
        if ui.button("Instantâneo salvo…").clicked() {
            if let Some(p) = rfd::FileDialog::new()
                .add_filter("Instantâneo", &["atlas"])
                .pick_file()
            {
                match snapshot::load(&p) {
                    Ok(s) => {
                        app.cmp.label = format!("{} ({})", p.display(), fmt::date(s.when));
                        app.cmp.old = Some(Arc::new(s));
                        app.cmp.dirty = true;
                    }
                    Err(e) => app.status = e.to_string(),
                }
            }
        }
        let hist: Vec<_> = app
            .history
            .iter()
            .filter(|h| h.when != scan.when)
            .cloned()
            .collect();
        egui::ComboBox::from_id_salt("hist")
            .selected_text("Varredura do histórico")
            .show_ui(ui, |ui| {
                for h in hist.iter().rev() {
                    if ui
                        .button(format!("{} — {}", fmt::date(h.when), app.sz(h.size)))
                        .clicked()
                    {
                        if let Ok(s) = snapshot::load(&h.file) {
                            app.cmp.label = format!("histórico de {}", fmt::date(h.when));
                            app.cmp.old = Some(Arc::new(s));
                            app.cmp.dirty = true;
                        }
                    }
                }
            });
        ui.label("Níveis");
        if ui
            .add(egui::DragValue::new(&mut app.cmp.depth).range(1..=30))
            .changed()
        {
            app.cmp.dirty = true;
        }
        if ui.checkbox(&mut app.cmp.files, "Arquivos").changed() {
            app.cmp.dirty = true;
        }
    });
    let Some(old) = app.cmp.old.clone() else {
        ui.label(RichText::new("Escolha um instantâneo ou uma varredura do histórico para ver onde o espaço cresceu ou diminuiu.").weak());
        return;
    };
    // Compara a partir da pasta selecionada, se ela existir no instantâneo antigo.
    let base = scan.path_of(&app.sel);
    let new_node = scan.root.get(&app.sel).unwrap_or(&scan.root);
    let old_node = snapshot::locate(&old, &base).unwrap_or(&old.root);
    if app.cmp.dirty {
        app.cmp.diffs = snapshot::compare(
            old_node,
            new_node,
            app.cmp.depth,
            app.cmp.files && !old.folders_only,
        );
        app.cmp.dirty = false;
    }
    let delta = new_node.size as i64 - old_node.size as i64;
    ui.horizontal(|ui| {
        ui.label(
            RichText::new(format!(
                "{}: {} → agora: {}",
                app.cmp.label,
                app.sz(old_node.size),
                app.sz(new_node.size)
            ))
            .strong(),
        );
        ui.label(
            RichText::new(fmt::signed(delta))
                .strong()
                .color(if delta > 0 {
                    Color32::from_rgb(0xd9, 0x4f, 0x4f)
                } else {
                    Color32::from_rgb(0x3b, 0x9e, 0x5f)
                }),
        );
        let diffs = app.cmp.diffs.clone();
        let title = format!("Comparação {} — {}", base.display(), app.cmp.label);
        export_menu(app, ui, "cmp", || snapshot::diff_table(title, &diffs));
    });
    if old.folders_only && app.cmp.files {
        ui.label(
            RichText::new("O histórico guarda só pastas; arquivos não são comparados.").weak(),
        );
    }
    let diffs = &app.cmp.diffs;
    ui.push_id("tabela-cmp", |ui| {
        TableBuilder::new(ui)
            .striped(true)
            .resizable(true)
            .column(Column::initial(110.0))
            .column(Column::initial(100.0))
            .column(Column::initial(100.0))
            .column(Column::initial(80.0))
            .column(Column::remainder().clip(true))
            .header(ROW, |mut h| {
                for t in ["Diferença", "Antes", "Depois", "Situação", "Caminho"] {
                    h.col(|ui| {
                        ui.strong(t);
                    });
                }
            })
            .body(|body| {
                body.rows(ROW, diffs.len(), |mut row| {
                    let d = &diffs[row.index()];
                    let col = if d.delta() > 0 {
                        Color32::from_rgb(0xd9, 0x4f, 0x4f)
                    } else {
                        Color32::from_rgb(0x3b, 0x9e, 0x5f)
                    };
                    row.col(|ui| {
                        ui.label(RichText::new(fmt::signed(d.delta())).color(col));
                    });
                    row.col(|ui| {
                        ui.label(app.sz(d.old_size));
                    });
                    row.col(|ui| {
                        ui.label(app.sz(d.new_size));
                    });
                    row.col(|ui| {
                        ui.label(match d.status {
                            Status::Added => "novo",
                            Status::Removed => "removido",
                            Status::Changed => "",
                        });
                    });
                    row.col(|ui| {
                        ui.label(format!(
                            "{}{}",
                            "  ".repeat(d.level.saturating_sub(1)),
                            d.path.display()
                        ));
                    });
                });
            });
    });
}

fn history_tab(app: &mut App, ui: &mut egui::Ui, scan: &Scan) {
    ui.horizontal(|ui| {
        ui.checkbox(
            &mut app.cfg.history,
            "Gravar no histórico após cada varredura",
        );
        ui.label(
            RichText::new("Guarda só as pastas, para acompanhar o crescimento ao longo do tempo.")
                .weak(),
        );
    });
    let hist = app.history.clone();
    if hist.is_empty() {
        ui.label(
            RichText::new(format!(
                "Ainda não há histórico para {}.",
                scan.root_path.display()
            ))
            .weak(),
        );
        return;
    }
    let items: Vec<(String, u64, Color32)> = hist
        .iter()
        .map(|h| {
            (
                fmt::date(h.when),
                h.size,
                Color32::from_rgb(0x3f, 0x8f, 0xe6),
            )
        })
        .collect();
    let f = |v: u64| app.sz(v);
    egui::CollapsingHeader::new("Tamanho ao longo do tempo")
        .default_open(true)
        .show(ui, |ui| treemap::bars_opt(ui, &items, &f, false));
    ui.separator();
    egui::ScrollArea::vertical().show(ui, |ui| {
        egui::Grid::new("hist")
            .striped(true)
            .num_columns(5)
            .spacing([16.0, 4.0])
            .show(ui, |ui| {
                for t in ["Data", "Tamanho", "Variação", "Arquivos", ""] {
                    ui.strong(t);
                }
                ui.end_row();
                let mut prev: Option<u64> = None;
                for h in &hist {
                    ui.label(fmt::date(h.when));
                    ui.label(app.sz(h.size));
                    ui.label(
                        prev.map(|p| fmt::signed(h.size as i64 - p as i64))
                            .unwrap_or_default(),
                    );
                    ui.label(fmt::int(h.files as i64));
                    if h.when != scan.when && ui.small_button("Comparar com a atual").clicked() {
                        if let Ok(s) = snapshot::load(&h.file) {
                            app.cmp.label = format!("histórico de {}", fmt::date(h.when));
                            app.cmp.old = Some(Arc::new(s));
                            app.cmp.dirty = true;
                            app.tab = Tab::Compare;
                        }
                    } else if h.when == scan.when {
                        ui.label(RichText::new("varredura atual").weak());
                    }
                    ui.end_row();
                    prev = Some(h.size);
                }
            });
    });
}

fn errors_tab(ui: &mut egui::Ui, scan: &Scan) {
    if scan.errors.is_empty() {
        ui.label("Nenhum erro na varredura.");
        return;
    }
    ui.label(format!(
        "{} pastas ou arquivos não puderam ser lidos (os totais não incluem o conteúdo deles).",
        scan.errors.len()
    ));
    if cfg!(windows) {
        ui.label(
            RichText::new("Dica: Arquivo › Reiniciar como administrador lê pastas protegidas.")
                .weak(),
        );
    }
    egui::ScrollArea::vertical()
        .auto_shrink([false; 2])
        .show_rows(ui, ROW, scan.errors.len(), |ui, range| {
            for e in &scan.errors[range] {
                ui.label(RichText::new(e).monospace());
            }
        });
}
