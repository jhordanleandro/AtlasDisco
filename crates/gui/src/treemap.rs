//! Mapa de árvore (layout "squarified") e gráficos simples desenhados com o painter do egui.

use atlasdisco_core::analysis::category;
use atlasdisco_core::fmt;
use atlasdisco_core::Node;
use eframe::egui::{self, Color32, Mesh, Pos2, Rect, Shape, Stroke, Vec2};

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Measure {
    /// Só o que ocupa este disco (sem arquivos que estão apenas na nuvem).
    Local,
    Size,
    Alloc,
    Files,
}

pub fn value(n: &Node, m: Measure) -> u64 {
    match m {
        Measure::Local => n.local(),
        Measure::Size => n.size,
        Measure::Alloc => n.alloc,
        Measure::Files => {
            if n.is_dir {
                n.files
            } else {
                1
            }
        }
    }
}

pub struct Tile {
    pub rect: Rect,
    pub idx: Vec<usize>,
    pub depth: usize,
    pub is_dir: bool,
    pub color: Color32,
}

pub struct Params {
    pub max_depth: usize,
    pub measure: Measure,
    pub files: bool,
    /// Mostra a hierarquia (molduras das pastas); desligado só desenha as folhas.
    pub hierarchy: bool,
}

/// Calcula os blocos de `node` dentro de `rect`. `base` é o índice do nó na árvore.
pub fn layout(node: &Node, base: &[usize], rect: Rect, p: &Params) -> Vec<Tile> {
    let mut out = Vec::new();
    let mut idx = base.to_vec();
    rec(node, &mut idx, rect, 0, p, &mut out);
    out
}

fn rec(
    node: &Node,
    idx: &mut Vec<usize>,
    rect: Rect,
    depth: usize,
    p: &Params,
    out: &mut Vec<Tile>,
) {
    let inner = if depth > 0 && p.hierarchy && rect.width() > 12.0 && rect.height() > 12.0 {
        // Faixa de título no topo da pasta quando há espaço.
        let top = if rect.height() > 40.0 && rect.width() > 60.0 {
            14.0
        } else {
            2.0
        };
        Rect::from_min_max(
            rect.min + Vec2::new(2.0, top),
            rect.max - Vec2::new(2.0, 2.0),
        )
    } else {
        rect
    };
    let mut items: Vec<(usize, f64)> = node
        .children
        .iter()
        .enumerate()
        .filter(|(_, c)| (c.is_dir || p.files) && value(c, p.measure) > 0)
        .map(|(i, c)| (i, value(c, p.measure) as f64))
        .collect();
    items.sort_by(|a, b| b.1.total_cmp(&a.1));
    // Arquivos não desenhados individualmente viram um bloco só "[arquivos]".
    if !p.files {
        let loose: u64 = node
            .children
            .iter()
            .filter(|c| !c.is_dir)
            .map(|c| value(c, p.measure))
            .sum();
        if loose > 0 {
            items.push((usize::MAX, loose as f64));
            items.sort_by(|a, b| b.1.total_cmp(&a.1));
        }
    }
    for (i, r) in squarify(&items, inner) {
        if r.width() < 1.0 || r.height() < 1.0 {
            continue;
        }
        if i == usize::MAX {
            out.push(Tile {
                rect: r,
                idx: idx.clone(),
                depth: depth + 1,
                is_dir: false,
                color: Color32::from_gray(150),
            });
            continue;
        }
        let c = &node.children[i];
        idx.push(i);
        if c.is_dir {
            let deeper = depth + 1 < p.max_depth
                && r.width() > 6.0
                && r.height() > 6.0
                && !c.children.is_empty();
            if p.hierarchy || !deeper {
                out.push(Tile {
                    rect: r,
                    idx: idx.clone(),
                    depth: depth + 1,
                    is_dir: true,
                    color: dir_color(depth + 1),
                });
            }
            if deeper {
                rec(c, idx, r, depth + 1, p, out);
            }
        } else {
            out.push(Tile {
                rect: r,
                idx: idx.clone(),
                depth: depth + 1,
                is_dir: false,
                color: file_color(&c.ext()),
            });
        }
        idx.pop();
    }
}

/// Algoritmo "squarified" (Bruls, Huizing, van Wijk): blocos o mais quadrados possível.
pub fn squarify(items: &[(usize, f64)], rect: Rect) -> Vec<(usize, Rect)> {
    let total: f64 = items.iter().map(|x| x.1).sum();
    let mut out = Vec::with_capacity(items.len());
    if total <= 0.0 || rect.width() <= 0.0 || rect.height() <= 0.0 {
        return out;
    }
    let scale = (rect.width() as f64 * rect.height() as f64) / total;
    let mut r = rect;
    let mut i = 0;
    while i < items.len() {
        let short = r.width().min(r.height()) as f64;
        let worst = |sum: f64, min: f64, max: f64| {
            let s2 = sum * sum;
            let w2 = short * short;
            (w2 * max / s2).max(s2 / (w2 * min))
        };
        let a0 = items[i].1 * scale;
        let (mut sum, mut min, mut max) = (a0, a0, a0);
        let mut best = worst(sum, min, max);
        let mut j = i + 1;
        while j < items.len() {
            let a = items[j].1 * scale;
            let w = worst(sum + a, min.min(a), max.max(a));
            if w > best {
                break;
            }
            best = w;
            sum += a;
            min = min.min(a);
            max = max.max(a);
            j += 1;
        }
        let horizontal = r.width() >= r.height();
        let thick = (sum
            / if horizontal {
                r.height() as f64
            } else {
                r.width() as f64
            }) as f32;
        let mut off = 0.0f32;
        for item in &items[i..j] {
            let len = ((item.1 * scale) / thick as f64) as f32;
            let tile = if horizontal {
                Rect::from_min_size(Pos2::new(r.min.x, r.min.y + off), Vec2::new(thick, len))
            } else {
                Rect::from_min_size(Pos2::new(r.min.x + off, r.min.y), Vec2::new(len, thick))
            };
            out.push((item.0, tile));
            off += len;
        }
        if horizontal {
            r.min.x += thick;
        } else {
            r.min.y += thick;
        }
        i = j;
    }
    out
}

pub fn dir_color(depth: usize) -> Color32 {
    let g = (80 + (depth * 18).min(120)) as u8;
    Color32::from_gray(g)
}

pub fn category_color(cat: &str) -> Color32 {
    match cat {
        "Imagens" => Color32::from_rgb(0x3b, 0x9e, 0x5f),
        "Vídeos" => Color32::from_rgb(0xd9, 0x4f, 0x4f),
        "Áudio" => Color32::from_rgb(0xe0, 0x8e, 0x2b),
        "Compactados" => Color32::from_rgb(0x8e, 0x5c, 0xc9),
        "Imagens de disco" => Color32::from_rgb(0xb0, 0x3a, 0x8c),
        "Programas" => Color32::from_rgb(0x3f, 0x6f, 0xd6),
        "Documentos" => Color32::from_rgb(0x2a, 0xa6, 0xb5),
        "Código" => Color32::from_rgb(0x7a, 0x9a, 0x2f),
        "Temporários" => Color32::from_rgb(0xc9, 0xb0, 0x2e),
        "Sem extensão" => Color32::from_rgb(0x88, 0x88, 0x88),
        _ => Color32::from_rgb(0x6c, 0x7a, 0x8c),
    }
}

pub fn file_color(ext: &str) -> Color32 {
    category_color(category(ext))
}

fn shade(c: Color32, f: f32) -> Color32 {
    let m = |v: u8| ((v as f32 * f).clamp(0.0, 255.0)) as u8;
    Color32::from_rgb(m(c.r()), m(c.g()), m(c.b()))
}

/// Bloco com efeito "almofada": claro no canto superior esquerdo, escuro no inferior direito.
pub fn cushion(painter: &egui::Painter, r: Rect, c: Color32) {
    let mut mesh = Mesh::default();
    mesh.colored_vertex(r.left_top(), shade(c, 1.35));
    mesh.colored_vertex(r.right_top(), shade(c, 1.05));
    mesh.colored_vertex(r.right_bottom(), shade(c, 0.7));
    mesh.colored_vertex(r.left_bottom(), shade(c, 1.0));
    mesh.add_triangle(0, 1, 2);
    mesh.add_triangle(0, 2, 3);
    painter.add(Shape::mesh(mesh));
}

pub fn paint(
    painter: &egui::Painter,
    tiles: &[Tile],
    root: &Node,
    highlight: Option<&[usize]>,
    base_len: usize,
) {
    let text = Color32::from_gray(245);
    for t in tiles {
        if t.is_dir {
            painter.rect_filled(t.rect, 0.0, t.color);
            painter.rect_stroke(
                t.rect,
                0.0,
                Stroke::new(1.0f32, Color32::from_gray(30)),
                egui::StrokeKind::Inside,
            );
            if t.rect.width() > 60.0 && t.rect.height() > 40.0 {
                if let Some(n) = root.get(&t.idx[base_len..]) {
                    let clip = painter.with_clip_rect(t.rect.shrink(1.0));
                    clip.text(
                        t.rect.min + Vec2::new(4.0, 1.0),
                        egui::Align2::LEFT_TOP,
                        &n.name,
                        egui::FontId::proportional(11.0),
                        text,
                    );
                }
            }
        } else {
            cushion(painter, t.rect, t.color);
            if t.rect.width() > 2.0 && t.rect.height() > 2.0 {
                painter.rect_stroke(
                    t.rect,
                    0.0,
                    Stroke::new(0.5f32, Color32::from_black_alpha(90)),
                    egui::StrokeKind::Inside,
                );
            }
            if t.rect.width() > 70.0 && t.rect.height() > 18.0 {
                let name = root
                    .get(&t.idx[base_len..])
                    .filter(|n| !n.is_dir)
                    .map(|n| n.name.as_str())
                    .unwrap_or("[arquivos]");
                let clip = painter.with_clip_rect(t.rect.shrink(1.0));
                clip.text(
                    t.rect.center(),
                    egui::Align2::CENTER_CENTER,
                    name,
                    egui::FontId::proportional(11.0),
                    text,
                );
            }
        }
    }
    if let Some(h) = highlight {
        if let Some(t) = tiles.iter().rev().find(|t| t.idx == h) {
            painter.rect_stroke(
                t.rect,
                0.0,
                Stroke::new(2.5f32, Color32::YELLOW),
                egui::StrokeKind::Inside,
            );
        }
    }
}

/// Bloco mais profundo sob o ponteiro.
pub fn hit(tiles: &[Tile], pos: Pos2) -> Option<&Tile> {
    tiles.iter().rev().find(|t| t.rect.contains(pos))
}

/// Gráfico de pizza com legenda. `items`: (rótulo, valor, cor).
pub fn pie(ui: &mut egui::Ui, items: &[(String, u64, Color32)], fmt_val: &dyn Fn(u64) -> String) {
    let total: u64 = items.iter().map(|x| x.1).sum();
    let avail = ui.available_size();
    let d = avail.y.min(avail.x * 0.55).clamp(120.0, 520.0);
    ui.horizontal(|ui| {
        let (rect, resp) = ui.allocate_exact_size(Vec2::splat(d), egui::Sense::hover());
        let painter = ui.painter_at(rect);
        let c = rect.center();
        let rad = d * 0.48;
        let mut a0 = -std::f32::consts::FRAC_PI_2;
        let mut hovered: Option<usize> = None;
        let pointer = resp.hover_pos();
        for (k, (_, v, col)) in items.iter().enumerate() {
            if total == 0 {
                break;
            }
            let sweep = *v as f32 / total as f32 * std::f32::consts::TAU;
            let steps = ((sweep / 0.05).ceil() as usize).max(1);
            let mut mesh = Mesh::default();
            mesh.colored_vertex(c, *col);
            for s in 0..=steps {
                let a = a0 + sweep * s as f32 / steps as f32;
                mesh.colored_vertex(c + Vec2::angled(a) * rad, shade(*col, 0.9));
            }
            for s in 0..steps as u32 {
                mesh.add_triangle(0, s + 1, s + 2);
            }
            painter.add(Shape::mesh(mesh));
            if let Some(p) = pointer {
                let v = p - c;
                let mut ang = v.y.atan2(v.x);
                while ang < a0 {
                    ang += std::f32::consts::TAU;
                }
                if v.length() <= rad && ang <= a0 + sweep {
                    hovered = Some(k);
                }
            }
            a0 += sweep;
        }
        if let Some(k) = hovered {
            resp.on_hover_text_at_pointer(format!(
                "{}\n{} ({})",
                items[k].0,
                fmt_val(items[k].1),
                fmt::pct(items[k].1 as f64 / total.max(1) as f64)
            ));
        }
        ui.vertical(|ui| legend(ui, items, total, fmt_val));
    });
}

pub fn legend(
    ui: &mut egui::Ui,
    items: &[(String, u64, Color32)],
    total: u64,
    fmt_val: &dyn Fn(u64) -> String,
) {
    egui::Grid::new(ui.next_auto_id())
        .num_columns(4)
        .spacing([10.0, 4.0])
        .show(ui, |ui| {
            for (label, v, col) in items {
                let (r, _) = ui.allocate_exact_size(Vec2::splat(12.0), egui::Sense::hover());
                ui.painter().rect_filled(r, 2.0, *col);
                ui.label(label);
                ui.label(fmt_val(*v));
                ui.label(fmt::pct(*v as f64 / total.max(1) as f64));
                ui.end_row();
            }
        });
}

/// Barras horizontais com rótulo, valor e porcentagem.
pub fn bars(ui: &mut egui::Ui, items: &[(String, u64, Color32)], fmt_val: &dyn Fn(u64) -> String) {
    bars_opt(ui, items, fmt_val, true)
}

/// `share`: mostra a fatia de cada item no total (desligado em séries temporais).
pub fn bars_opt(
    ui: &mut egui::Ui,
    items: &[(String, u64, Color32)],
    fmt_val: &dyn Fn(u64) -> String,
    share: bool,
) {
    let max = items.iter().map(|x| x.1).max().unwrap_or(1).max(1);
    let total: u64 = items.iter().map(|x| x.1).sum();
    egui::Grid::new(ui.next_auto_id())
        .num_columns(3)
        .spacing([10.0, 6.0])
        .show(ui, |ui| {
            for (label, v, col) in items {
                ui.label(label);
                let w = (ui.available_width() - 160.0).clamp(120.0, 600.0);
                let (r, resp) = ui.allocate_exact_size(Vec2::new(w, 16.0), egui::Sense::hover());
                ui.painter()
                    .rect_filled(r, 2.0, ui.visuals().faint_bg_color);
                let fill =
                    Rect::from_min_size(r.min, Vec2::new(w * (*v as f32 / max as f32), r.height()));
                ui.painter().rect_filled(fill, 2.0, *col);
                let p = fmt::pct(*v as f64 / total.max(1) as f64);
                resp.on_hover_text(p.clone());
                ui.label(if share {
                    format!("{}  ({p})", fmt_val(*v))
                } else {
                    fmt_val(*v)
                });
                ui.end_row();
            }
        });
}

pub const PALETTE: [Color32; 12] = [
    Color32::from_rgb(0x3f, 0x6f, 0xd6),
    Color32::from_rgb(0xd9, 0x4f, 0x4f),
    Color32::from_rgb(0x3b, 0x9e, 0x5f),
    Color32::from_rgb(0xe0, 0x8e, 0x2b),
    Color32::from_rgb(0x8e, 0x5c, 0xc9),
    Color32::from_rgb(0x2a, 0xa6, 0xb5),
    Color32::from_rgb(0xb0, 0x3a, 0x8c),
    Color32::from_rgb(0x7a, 0x9a, 0x2f),
    Color32::from_rgb(0xc9, 0xb0, 0x2e),
    Color32::from_rgb(0x5a, 0x7d, 0x9a),
    Color32::from_rgb(0xa0, 0x6a, 0x4a),
    Color32::from_rgb(0x88, 0x88, 0x88),
];

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn squarify_cobre_a_area() {
        let r = Rect::from_min_size(Pos2::ZERO, Vec2::new(600.0, 400.0));
        let items = vec![
            (0, 6.0),
            (1, 6.0),
            (2, 4.0),
            (3, 3.0),
            (4, 2.0),
            (5, 2.0),
            (6, 1.0),
        ];
        let t = squarify(&items, r);
        assert_eq!(t.len(), 7);
        let area: f32 = t.iter().map(|(_, r)| r.area()).sum();
        assert!((area - r.area()).abs() < 1.0, "{area}");
        for (_, x) in &t {
            assert!(r.expand(0.5).contains_rect(*x));
        }
        // proporcional ao valor
        assert!((t[0].1.area() / r.area() - 6.0 / 24.0).abs() < 0.01);
    }
}
