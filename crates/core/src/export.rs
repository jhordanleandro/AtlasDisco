//! Exportação de tabelas: CSV, TXT, HTML, Excel (.xlsx), JSON, SQLite e PDF.

use crate::fmt;
use crate::model::{Node, Scan};
use std::fs;
use std::io::Write;
use std::path::Path;

#[derive(Clone, Debug)]
pub enum Cell {
    Text(String),
    Int(i64),
    Bytes(u64),
    Pct(f64),
    Date(i64),
}

impl Cell {
    pub fn display(&self) -> String {
        match self {
            Cell::Text(s) => s.clone(),
            Cell::Int(n) => fmt::int(*n),
            Cell::Bytes(b) => fmt::size(*b),
            Cell::Pct(p) => fmt::pct(*p),
            Cell::Date(t) => fmt::date(*t),
        }
    }
    /// Valor bruto (bytes sem unidade, data ISO) para formatos de dados.
    pub fn raw(&self) -> String {
        match self {
            Cell::Text(s) => s.clone(),
            Cell::Int(n) => n.to_string(),
            Cell::Bytes(b) => b.to_string(),
            Cell::Pct(p) => format!("{:.4}", p),
            Cell::Date(t) => iso(*t),
        }
    }
}

fn iso(t: i64) -> String {
    if t <= 0 {
        return String::new();
    }
    chrono::DateTime::from_timestamp(t, 0)
        .map(|d| d.format("%Y-%m-%dT%H:%M:%SZ").to_string())
        .unwrap_or_default()
}

#[derive(Clone, Debug, Default)]
pub struct Table {
    pub title: String,
    pub headers: Vec<String>,
    pub rows: Vec<Vec<Cell>>,
    /// Recuo por linha (nível na árvore) para TXT/HTML/PDF.
    pub indent: Vec<usize>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Format {
    Csv,
    Txt,
    Html,
    Xlsx,
    Json,
    Sqlite,
    Pdf,
}

impl Format {
    pub const ALL: [Format; 7] = [
        Format::Xlsx,
        Format::Csv,
        Format::Html,
        Format::Pdf,
        Format::Txt,
        Format::Json,
        Format::Sqlite,
    ];
    pub fn ext(self) -> &'static str {
        match self {
            Format::Csv => "csv",
            Format::Txt => "txt",
            Format::Html => "html",
            Format::Xlsx => "xlsx",
            Format::Json => "json",
            Format::Sqlite => "sqlite",
            Format::Pdf => "pdf",
        }
    }
    pub fn name(self) -> &'static str {
        match self {
            Format::Csv => "CSV (Excel, ponto e vírgula)",
            Format::Txt => "Texto",
            Format::Html => "HTML",
            Format::Xlsx => "Excel (.xlsx)",
            Format::Json => "JSON",
            Format::Sqlite => "Banco SQLite",
            Format::Pdf => "PDF",
        }
    }
    pub fn from_path(p: &Path) -> Option<Format> {
        let e = p.extension()?.to_string_lossy().to_lowercase();
        Format::ALL.into_iter().find(|f| {
            f.ext() == e
                || (e == "db" && *f == Format::Sqlite)
                || (e == "htm" && *f == Format::Html)
        })
    }
}

/// Tabela da árvore a partir do nó `node` (caminho `base`) até `depth` níveis.
pub fn tree_table(node: &Node, base: &Path, depth: usize, files: bool) -> Table {
    let mut t = Table {
        title: format!("AtlasDisco — {}", base.display()),
        headers: [
            "Caminho",
            "Nome",
            "Nível",
            "Tipo",
            "Tamanho",
            "Alocado",
            "Na nuvem",
            "% do pai",
            "Arquivos",
            "Pastas",
            "Última modificação",
            "Último acesso",
            "Criação",
            "Atributos",
        ]
        .map(String::from)
        .to_vec(),
        ..Default::default()
    };
    fn rec(n: &Node, p: &Path, lvl: usize, parent: u64, depth: usize, files: bool, t: &mut Table) {
        t.rows.push(vec![
            Cell::Text(p.to_string_lossy().into_owned()),
            Cell::Text(if lvl == 0 {
                p.to_string_lossy().into_owned()
            } else {
                n.name.clone()
            }),
            Cell::Int(lvl as i64),
            Cell::Text(if n.is_dir { "Pasta" } else { "Arquivo" }.into()),
            Cell::Bytes(n.size),
            Cell::Bytes(n.alloc),
            Cell::Bytes(n.cloud),
            Cell::Pct(if parent == 0 {
                1.0
            } else {
                n.size as f64 / parent as f64
            }),
            Cell::Int(if n.is_dir { n.files as i64 } else { 1 }),
            Cell::Int(n.dirs as i64),
            Cell::Date(n.mtime),
            Cell::Date(n.atime),
            Cell::Date(n.ctime),
            Cell::Text(fmt::attrs(n.attrs)),
        ]);
        t.indent.push(lvl);
        if lvl < depth {
            for c in &n.children {
                if c.is_dir || files {
                    rec(c, &p.join(&c.name), lvl + 1, n.size, depth, files, t);
                }
            }
        }
    }
    rec(node, base, 0, 0, depth, files, &mut t);
    t
}

/// Resumo de uma varredura para o cabeçalho dos relatórios.
pub fn summary(scan: &Scan) -> String {
    format!(
        "{} — {} em {} arquivos e {} pastas; varrido em {}",
        scan.root_path.display(),
        fmt::size(scan.root.size),
        fmt::int(scan.root.files as i64),
        fmt::int(scan.root.dirs as i64),
        fmt::date(scan.when)
    )
}

pub fn write(t: &Table, path: &Path, format: Format) -> Result<(), String> {
    let r = match format {
        Format::Csv => csv(t, path),
        Format::Txt => txt(t, path),
        Format::Html => html(t, path),
        Format::Json => json(t, path),
        Format::Xlsx => return xlsx(t, path),
        Format::Sqlite => return sqlite(t, path),
        Format::Pdf => pdf(t, path),
    };
    r.map_err(|e| e.to_string())
}

fn csv(t: &Table, path: &Path) -> std::io::Result<()> {
    let q = |s: &str| {
        if s.contains([';', '"', '\n']) {
            format!("\"{}\"", s.replace('"', "\"\""))
        } else {
            s.to_string()
        }
    };
    let mut out = String::from("\u{feff}"); // BOM: o Excel reconhece UTF-8
    out += &t.headers.iter().map(|h| q(h)).collect::<Vec<_>>().join(";");
    out.push_str("\r\n");
    for r in &t.rows {
        // Números crus com vírgula decimal: o Excel em pt-BR soma e ordena.
        let cells: Vec<String> = r
            .iter()
            .map(|c| match c {
                Cell::Pct(p) => fmt::dec_br(p * 100.0, 2),
                Cell::Date(_) => c.display(),
                _ => q(&c.raw()),
            })
            .collect();
        out += &cells.join(";");
        out.push_str("\r\n");
    }
    fs::write(path, out)
}

fn txt(t: &Table, path: &Path) -> std::io::Result<()> {
    let mut out = format!("{}\r\n\r\n", t.title);
    let widths: Vec<usize> = (0..t.headers.len())
        .map(|i| {
            t.rows
                .iter()
                .map(|r| r[i].display().chars().count())
                .chain([t.headers[i].chars().count()])
                .max()
                .unwrap_or(0)
                .min(80)
        })
        .collect();
    let line = |cells: Vec<String>| {
        cells
            .iter()
            .zip(&widths)
            .map(|(c, w)| format!("{c:<w$}"))
            .collect::<Vec<_>>()
            .join("  ")
            .trim_end()
            .to_string()
    };
    out += &line(t.headers.clone());
    out.push_str("\r\n");
    for (i, r) in t.rows.iter().enumerate() {
        let mut cells: Vec<String> = r.iter().map(Cell::display).collect();
        if let Some(&lvl) = t.indent.get(i) {
            if cells.len() > 1 {
                cells[1] = format!("{}{}", "  ".repeat(lvl), cells[1]);
            }
        }
        out += &line(cells);
        out.push_str("\r\n");
    }
    fs::write(path, out)
}

fn esc(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

fn html(t: &Table, path: &Path) -> std::io::Result<()> {
    let size_col = t.headers.iter().position(|h| h == "Tamanho");
    let pct_col = t.headers.iter().position(|h| h == "% do pai");
    let mut o = String::new();
    o += "<!doctype html><html lang=\"pt-BR\"><meta charset=\"utf-8\"><meta name=\"viewport\" content=\"width=device-width,initial-scale=1\">";
    o += &format!("<title>{}</title>", esc(&t.title));
    o += "<style>body{font:14px system-ui,sans-serif;margin:16px;color:#1d2433;background:#fff}h1{font-size:18px}\
          table{border-collapse:collapse;width:100%}th,td{padding:4px 8px;border-bottom:1px solid #e3e6ec;text-align:left;white-space:nowrap}\
          th{background:#f3f5f8;position:sticky;top:0}td.n{text-align:right;font-variant-numeric:tabular-nums}\
          .bar{display:inline-block;height:10px;background:#3b82f6;border-radius:2px;vertical-align:middle;margin-right:6px}\
          @media(prefers-color-scheme:dark){body{background:#14171c;color:#e6e8eb}th{background:#1f242c}th,td{border-color:#2a3038}}</style>";
    o += &format!(
        "<h1>{}</h1><p>Gerado em {} pelo AtlasDisco</p><table><thead><tr>",
        esc(&t.title),
        fmt::date(chrono::Utc::now().timestamp())
    );
    for h in &t.headers {
        o += &format!("<th>{}</th>", esc(h));
    }
    o += "</tr></thead><tbody>\n";
    for (i, r) in t.rows.iter().enumerate() {
        o += "<tr>";
        for (j, c) in r.iter().enumerate() {
            let num = matches!(c, Cell::Int(_) | Cell::Bytes(_) | Cell::Pct(_));
            let mut v = esc(&c.display());
            if j == 1 {
                if let Some(&lvl) = t.indent.get(i) {
                    v = format!("<span style=\"padding-left:{}px\">{v}</span>", lvl * 16);
                }
            }
            if Some(j) == size_col {
                if let Some(Cell::Pct(p)) = pct_col.and_then(|k| r.get(k)) {
                    v = format!(
                        "<span class=\"bar\" style=\"width:{}px\"></span>{v}",
                        (p * 80.0).round()
                    );
                }
            }
            o += &format!("<td{}>{v}</td>", if num { " class=\"n\"" } else { "" });
        }
        o += "</tr>\n";
    }
    o += "</tbody></table></html>";
    fs::write(path, o)
}

fn json(t: &Table, path: &Path) -> std::io::Result<()> {
    let rows: Vec<serde_json::Map<String, serde_json::Value>> = t
        .rows
        .iter()
        .map(|r| {
            t.headers
                .iter()
                .zip(r)
                .map(|(h, c)| {
                    let v = match c {
                        Cell::Int(n) => serde_json::json!(n),
                        Cell::Bytes(b) => serde_json::json!(b),
                        Cell::Pct(p) => serde_json::json!(p),
                        _ => serde_json::json!(c.raw()),
                    };
                    (h.clone(), v)
                })
                .collect()
        })
        .collect();
    let doc = serde_json::json!({ "titulo": t.title, "linhas": rows });
    fs::write(path, serde_json::to_vec_pretty(&doc)?)
}

fn xlsx(t: &Table, path: &Path) -> Result<(), String> {
    use rust_xlsxwriter::{Format as XF, Workbook};
    let mut wb = Workbook::new();
    let ws = wb.add_worksheet();
    let e = |e: rust_xlsxwriter::XlsxError| e.to_string();
    ws.set_name("AtlasDisco").map_err(e)?;
    let bold = XF::new().set_bold().set_background_color(0xE8EDF5);
    let bytes = XF::new().set_num_format("#,##0");
    let pct = XF::new().set_num_format("0.0%");
    let date = XF::new().set_num_format("dd/mm/yyyy hh:mm");
    ws.write_string(0, 0, &t.title).map_err(e)?;
    for (j, h) in t.headers.iter().enumerate() {
        ws.write_string_with_format(2, j as u16, h, &bold)
            .map_err(e)?;
    }
    for (i, r) in t.rows.iter().enumerate() {
        let row = (i + 3) as u32;
        for (j, c) in r.iter().enumerate() {
            let col = j as u16;
            match c {
                Cell::Text(s) => ws.write_string(row, col, s).map(|_| ()),
                Cell::Int(n) => ws.write_number(row, col, *n as f64).map(|_| ()),
                Cell::Bytes(b) => ws
                    .write_number_with_format(row, col, *b as f64, &bytes)
                    .map(|_| ()),
                Cell::Pct(p) => ws.write_number_with_format(row, col, *p, &pct).map(|_| ()),
                Cell::Date(ts) if *ts > 0 => {
                    // Número de série do Excel (dias desde 1899-12-30), no fuso local.
                    let off = chrono::Local::now().offset().local_minus_utc() as i64;
                    let serial = (*ts + off) as f64 / 86400.0 + 25569.0;
                    ws.write_number_with_format(row, col, serial, &date)
                        .map(|_| ())
                }
                Cell::Date(_) => Ok(()),
            }
            .map_err(e)?;
        }
    }
    ws.set_freeze_panes(3, 0).map_err(e)?;
    ws.autofit();
    if !t.rows.is_empty() {
        ws.autofilter(
            2,
            0,
            (t.rows.len() + 2) as u32,
            (t.headers.len() - 1) as u16,
        )
        .map_err(e)?;
    }
    wb.save(path).map_err(e)
}

fn sqlite(t: &Table, path: &Path) -> Result<(), String> {
    let _ = fs::remove_file(path);
    let mut db = rusqlite::Connection::open(path).map_err(|e| e.to_string())?;
    let cols: Vec<String> = t
        .headers
        .iter()
        .map(|h| format!("\"{}\"", h.replace('"', "\"\"")))
        .collect();
    let types: Vec<&str> = t
        .rows
        .first()
        .map(|r| {
            r.iter()
                .map(|c| match c {
                    Cell::Int(_) | Cell::Bytes(_) => "INTEGER",
                    Cell::Pct(_) => "REAL",
                    _ => "TEXT",
                })
                .collect()
        })
        .unwrap_or_else(|| vec!["TEXT"; cols.len()]);
    let ddl = format!(
        "CREATE TABLE itens ({})",
        cols.iter()
            .zip(&types)
            .map(|(c, t)| format!("{c} {t}"))
            .collect::<Vec<_>>()
            .join(", ")
    );
    db.execute_batch(&ddl).map_err(|e| e.to_string())?;
    db.execute("CREATE TABLE info (chave TEXT, valor TEXT)", [])
        .map_err(|e| e.to_string())?;
    db.execute("INSERT INTO info VALUES ('titulo', ?1)", [&t.title])
        .map_err(|e| e.to_string())?;
    let tx = db.transaction().map_err(|e| e.to_string())?;
    {
        let sql = format!(
            "INSERT INTO itens VALUES ({})",
            vec!["?"; cols.len()].join(",")
        );
        let mut st = tx.prepare(&sql).map_err(|e| e.to_string())?;
        for r in &t.rows {
            let vals: Vec<rusqlite::types::Value> = r
                .iter()
                .map(|c| match c {
                    Cell::Int(n) => rusqlite::types::Value::Integer(*n),
                    Cell::Bytes(b) => rusqlite::types::Value::Integer(*b as i64),
                    Cell::Pct(p) => rusqlite::types::Value::Real(*p),
                    _ => rusqlite::types::Value::Text(c.raw()),
                })
                .collect();
            st.execute(rusqlite::params_from_iter(vals))
                .map_err(|e| e.to_string())?;
        }
    }
    tx.commit().map_err(|e| e.to_string())
}

// ---------------------------------------------------------------- PDF mínimo (texto, A4 paisagem)

fn pdf(t: &Table, path: &Path) -> std::io::Result<()> {
    // Colunas que cabem na página: nome, tamanho, alocado, %, arquivos, pastas, modificação.
    let want = [
        "Nome",
        "Tamanho",
        "Alocado",
        "% do pai",
        "Arquivos",
        "Pastas",
        "Última modificação",
    ];
    let mut idx: Vec<usize> = want
        .iter()
        .filter_map(|w| t.headers.iter().position(|h| h == w))
        .collect();
    if idx.is_empty() {
        idx = (0..t.headers.len().min(7)).collect();
    }
    let widths: Vec<f32> = idx
        .iter()
        .enumerate()
        .map(|(k, _)| if k == 0 { 300.0 } else { 78.0 })
        .collect();
    let (pw, ph, margin, lh, fsz) = (842.0f32, 595.0f32, 36.0f32, 12.0f32, 8.0f32);
    let per_page = ((ph - 2.0 * margin - 40.0) / lh) as usize;
    let mut pages: Vec<String> = Vec::new();
    let chunks: Vec<&[Vec<Cell>]> = if t.rows.is_empty() {
        vec![&[][..]]
    } else {
        t.rows.chunks(per_page).collect()
    };
    for (pi, chunk) in chunks.iter().enumerate() {
        let mut s = String::new();
        let mut y = ph - margin;
        s += &format!(
            "BT /F2 12 Tf {margin} {y} Td ({}) Tj ET\n",
            pdf_str(&t.title)
        );
        y -= 18.0;
        s += &format!(
            "BT /F1 7 Tf {} {} Td (Pagina {} de {}) Tj ET\n",
            pw - margin - 60.0,
            ph - margin,
            pi + 1,
            chunks.len()
        );
        let mut x = margin;
        for (k, &c) in idx.iter().enumerate() {
            s += &format!(
                "BT /F2 {fsz} Tf {x} {y} Td ({}) Tj ET\n",
                pdf_str(&t.headers[c])
            );
            x += widths[k];
        }
        y -= 4.0;
        s += &format!("0.7 G {margin} {y} m {} {y} l S 0 G\n", pw - margin);
        y -= lh;
        for (ri, r) in chunk.iter().enumerate() {
            let row_index = pi * per_page + ri;
            let mut x = margin;
            for (k, &c) in idx.iter().enumerate() {
                let mut v = r[c].display();
                if k == 0 {
                    let lvl = t.indent.get(row_index).copied().unwrap_or(0);
                    v = format!("{}{}", "   ".repeat(lvl.min(20)), v);
                    if v.chars().count() > 70 {
                        v = v.chars().take(69).collect::<String>() + "…";
                    }
                }
                let numeric = matches!(r[c], Cell::Int(_) | Cell::Bytes(_) | Cell::Pct(_));
                let tx = if numeric {
                    x + widths[k] - 8.0 - v.chars().count() as f32 * fsz * 0.5
                } else {
                    x
                };
                s += &format!(
                    "BT /F1 {fsz} Tf {tx:.1} {y:.1} Td ({}) Tj ET\n",
                    pdf_str(&v)
                );
                x += widths[k];
            }
            y -= lh;
        }
        pages.push(s);
    }
    // Monta objetos: 1 catálogo, 2 páginas, 3 fonte normal, 4 fonte negrito, depois (página, conteúdo) por página.
    let mut objs: Vec<Vec<u8>> = Vec::new();
    let n = pages.len();
    let kids: Vec<String> = (0..n).map(|i| format!("{} 0 R", 5 + 2 * i)).collect();
    objs.push(b"<< /Type /Catalog /Pages 2 0 R >>".to_vec());
    objs.push(format!("<< /Type /Pages /Kids [{}] /Count {n} >>", kids.join(" ")).into_bytes());
    objs.push(
        b"<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica /Encoding /WinAnsiEncoding >>"
            .to_vec(),
    );
    objs.push(
        b"<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica-Bold /Encoding /WinAnsiEncoding >>"
            .to_vec(),
    );
    for (i, p) in pages.iter().enumerate() {
        objs.push(
            format!("<< /Type /Page /Parent 2 0 R /MediaBox [0 0 {pw} {ph}] /Resources << /Font << /F1 3 0 R /F2 4 0 R >> >> /Contents {} 0 R >>", 6 + 2 * i)
                .into_bytes(),
        );
        let body = cp1252(p);
        let mut o = format!("<< /Length {} >>\nstream\n", body.len()).into_bytes();
        o.extend(body);
        o.extend(b"\nendstream");
        objs.push(o);
    }
    let mut out: Vec<u8> = b"%PDF-1.4\n%\xe2\xe3\xcf\xd3\n".to_vec();
    let mut offs = Vec::new();
    for (i, o) in objs.iter().enumerate() {
        offs.push(out.len());
        out.extend(format!("{} 0 obj\n", i + 1).bytes());
        out.extend(o);
        out.extend(b"\nendobj\n");
    }
    let xref = out.len();
    out.extend(format!("xref\n0 {}\n0000000000 65535 f \n", objs.len() + 1).bytes());
    for o in offs {
        out.extend(format!("{o:010} 00000 n \n").bytes());
    }
    out.extend(
        format!(
            "trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n",
            objs.len() + 1
        )
        .bytes(),
    );
    let mut f = fs::File::create(path)?;
    f.write_all(&out)
}

fn pdf_str(s: &str) -> String {
    s.replace('\\', "\\\\")
        .replace('(', "\\(")
        .replace(')', "\\)")
}

/// UTF-8 → Windows-1252 (fontes padrão do PDF). Caracteres fora do conjunto viram '?'.
fn cp1252(s: &str) -> Vec<u8> {
    s.chars()
        .map(|c| match c as u32 {
            0..=0x7f | 0xa0..=0xff => c as u32 as u8,
            0x2026 => 0x85,
            0x2212 | 0x2013 => 0x96,
            0x2014 => 0x97,
            0x20ac => 0x80,
            _ => b'?',
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn todos_os_formatos() {
        let mut root = Node {
            name: "r".into(),
            is_dir: true,
            ..Default::default()
        };
        root.children = vec![
            Node {
                name: "ação (1).txt".into(),
                size: 2048,
                alloc: 4096,
                mtime: 1_700_000_000,
                ..Default::default()
            },
            Node {
                name: "sub".into(),
                is_dir: true,
                ..Default::default()
            },
        ];
        root.recompute();
        let t = tree_table(&root, Path::new("C:\\r"), 3, true);
        assert_eq!(t.rows.len(), 3);
        let d = tempfile::tempdir().unwrap();
        for f in Format::ALL {
            let p = d.path().join(format!("x.{}", f.ext()));
            write(&t, &p, f).unwrap();
            assert!(fs::metadata(&p).unwrap().len() > 50, "{f:?}");
            assert_eq!(Format::from_path(&p), Some(f));
        }
        let csv = fs::read_to_string(d.path().join("x.csv")).unwrap();
        assert!(csv.contains("ação (1).txt;"));
        let pdf = fs::read(d.path().join("x.pdf")).unwrap();
        assert!(pdf.starts_with(b"%PDF-1.4") && pdf.ends_with(b"%%EOF\n"));
        let db = rusqlite::Connection::open(d.path().join("x.sqlite")).unwrap();
        let n: i64 = db
            .query_row("SELECT SUM(Tamanho) FROM itens WHERE Nível = 1", [], |r| {
                r.get(0)
            })
            .unwrap();
        assert_eq!(n, 2048);
    }
}
