//! atlasdisco-cli: uso em scripts, servidores e varreduras agendadas.

use atlasdisco_core::export::{self, Format, Table};
use atlasdisco_core::pattern::Matcher;
use atlasdisco_core::search::{self, Criteria, DupMode, DupProgress, Kind};
use atlasdisco_core::{analysis, fmt, ops, snapshot, Progress, Scan, ScanOptions};
use clap::{Args, Parser, Subcommand, ValueEnum};
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::sync::atomic::AtomicBool;

#[derive(Parser)]
#[command(
    name = "atlasdisco-cli",
    version,
    about = "Analisador de espaço em disco (linha de comando)"
)]
struct Cli {
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// Varre uma pasta, mostra os maiores itens e exporta relatórios
    Varrer {
        pasta: PathBuf,
        #[command(flatten)]
        scan: ScanArgs,
        /// Níveis de pasta no relatório
        #[arg(long, default_value_t = 3)]
        profundidade: usize,
        /// Incluir arquivos (não só pastas) no relatório
        #[arg(long)]
        arquivos: bool,
        /// Arquivo de saída (.xlsx .csv .html .pdf .txt .json .sqlite); {data} vira AAAA-MM-DD_HHMM. Pode repetir.
        #[arg(long, short = 'e')]
        exportar: Vec<PathBuf>,
        /// Salvar instantâneo completo (.atlas) para comparar depois
        #[arg(long)]
        salvar: Option<PathBuf>,
        /// Gravar no histórico da pasta (crescimento ao longo do tempo)
        #[arg(long)]
        historico: bool,
        /// Quantos maiores arquivos listar
        #[arg(long, default_value_t = 10)]
        top: usize,
        #[command(flatten)]
        email: EmailArgs,
    },
    /// Busca arquivos/pastas por nome, tamanho, data, caminho, nível e conteúdo
    Buscar {
        pasta: PathBuf,
        #[command(flatten)]
        scan: ScanArgs,
        /// Nome (curingas separados por ';', ex.: "*.tmp;*.bak")
        #[arg(long, default_value = "")]
        nome: String,
        /// O nome é uma expressão regular
        #[arg(long)]
        regex: bool,
        #[arg(long, value_enum, default_value_t = Tipo::Arquivos)]
        tipo: Tipo,
        /// Tamanho mínimo, ex.: 500MB
        #[arg(long)]
        min: Option<String>,
        #[arg(long)]
        max: Option<String>,
        /// Não modificado há pelo menos N dias
        #[arg(long)]
        antigo_dias: Option<i64>,
        /// Modificado nos últimos N dias
        #[arg(long)]
        recente_dias: Option<i64>,
        /// Caminho com pelo menos N caracteres (caminhos longos: 260)
        #[arg(long)]
        caminho_min: Option<usize>,
        /// Texto contido no arquivo
        #[arg(long, default_value = "")]
        conteudo: String,
        /// Carregar critérios de um modelo JSON salvo pela interface
        #[arg(long)]
        modelo: Option<PathBuf>,
        #[arg(long, default_value_t = 50)]
        limite: usize,
        #[arg(long, short = 'e')]
        exportar: Vec<PathBuf>,
    },
    /// Encontra arquivos duplicados
    Duplicados {
        pasta: PathBuf,
        #[command(flatten)]
        scan: ScanArgs,
        /// Tamanho mínimo dos arquivos, ex.: 1MB
        #[arg(long, default_value = "1KB")]
        min: String,
        #[arg(long, value_enum, default_value_t = Modo::Sha256)]
        modo: Modo,
        #[arg(long, short = 'e')]
        exportar: Vec<PathBuf>,
    },
    /// Compara duas varreduras (instantâneo .atlas ou pasta atual)
    Comparar {
        antigo: PathBuf,
        novo: PathBuf,
        #[arg(long, default_value_t = 4)]
        profundidade: usize,
        #[arg(long)]
        arquivos: bool,
        #[arg(long, default_value_t = 30)]
        limite: usize,
        #[arg(long, short = 'e')]
        exportar: Vec<PathBuf>,
    },
    /// Lista o histórico de varreduras de uma pasta
    Historico { pasta: PathBuf },
    /// Agenda uma varredura periódica (Agendador de Tarefas no Windows; crontab nos demais)
    Agendar {
        /// Nome da tarefa
        nome: String,
        #[arg(long, value_enum, default_value_t = Freq::Diario)]
        frequencia: Freq,
        /// Horário HH:MM
        #[arg(long, default_value = "03:00")]
        hora: String,
        /// Argumentos do "varrer", ex.: -- varrer D:\ --historico -e C:\rel\{data}.xlsx
        #[arg(last = true, required = true)]
        comando: Vec<String>,
    },
    /// Remove uma varredura agendada (Windows)
    Desagendar { nome: String },
}

#[derive(Clone, Copy, ValueEnum)]
enum Tipo {
    Arquivos,
    Pastas,
    Ambos,
}
#[derive(Clone, Copy, ValueEnum)]
enum Modo {
    NomeTamanho,
    Md5,
    Sha256,
}
#[derive(Clone, Copy, ValueEnum)]
enum Freq {
    Diario,
    Semanal,
    Mensal,
}

#[derive(Args, Clone, Default)]
struct ScanArgs {
    /// Arquivos/pastas a ignorar (curingas separados por ';')
    #[arg(long, default_value = "")]
    excluir: String,
    /// Só arquivos que casam (curingas separados por ';')
    #[arg(long, default_value = "")]
    incluir: String,
    /// Seguir links simbólicos e junções
    #[arg(long)]
    seguir_links: bool,
    /// Contar arquivos com vários hardlinks uma única vez
    #[arg(long)]
    hardlinks: bool,
    /// Somar fluxos alternativos NTFS (ADS)
    #[arg(long)]
    ads: bool,
    /// Ignorar arquivos que estão só na nuvem (OneDrive sob demanda etc.)
    #[arg(long)]
    sem_nuvem: bool,
    #[arg(long)]
    threads: Option<usize>,
}

#[derive(Args)]
struct EmailArgs {
    /// Enviar os relatórios exportados para este endereço (senha em ATLASDISCO_SMTP_SENHA)
    #[arg(long)]
    email_para: Option<String>,
    #[arg(long)]
    email_de: Option<String>,
    /// Servidor SMTP (porta 465 = TLS; outras = STARTTLS)
    #[arg(long)]
    smtp: Option<String>,
    #[arg(long)]
    smtp_usuario: Option<String>,
}

fn main() -> ExitCode {
    match run(Cli::parse()) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("erro: {e}");
            ExitCode::FAILURE
        }
    }
}

fn options(a: &ScanArgs) -> Result<ScanOptions, String> {
    let mut o = ScanOptions {
        exclude: Matcher::new(&a.excluir, false).map_err(|e| e.to_string())?,
        include: Matcher::new(&a.incluir, false).map_err(|e| e.to_string())?,
        follow_links: a.seguir_links,
        dedup_hardlinks: a.hardlinks,
        streams: a.ads,
        skip_cloud: a.sem_nuvem,
        ..Default::default()
    };
    if let Some(t) = a.threads {
        o.threads = t;
    }
    Ok(o)
}

fn do_scan(p: &Path, a: &ScanArgs) -> Result<Scan, String> {
    eprintln!("Varrendo {} ...", p.display());
    let s = atlasdisco_core::scan(p, &options(a)?, &Progress::default())
        .map_err(|e| format!("{}: {e}", p.display()))?;
    eprintln!(
        "{}  ({} arquivos, {} pastas, {:.1} s, {} erros)",
        fmt::size(s.root.size),
        fmt::int(s.root.files as i64),
        fmt::int(s.root.dirs as i64),
        s.seconds,
        s.errors.len()
    );
    if s.root.cloud > 0 {
        eprintln!(
            "Só na nuvem (não ocupa o disco): {}; local: {}",
            fmt::size(s.root.cloud),
            fmt::size(s.root.local())
        );
    }
    Ok(s)
}

fn out_path(p: &Path) -> PathBuf {
    let stamp = chrono::Local::now().format("%Y-%m-%d_%H%M").to_string();
    PathBuf::from(p.to_string_lossy().replace("{data}", &stamp))
}

fn export_all(t: &Table, outs: &[PathBuf]) -> Result<Vec<PathBuf>, String> {
    let mut done = Vec::new();
    for o in outs {
        let o = out_path(o);
        let f = Format::from_path(&o).ok_or_else(|| {
            format!(
                "{}: extensão não reconhecida (use .xlsx .csv .html .pdf .txt .json .sqlite)",
                o.display()
            )
        })?;
        if let Some(d) = o.parent().filter(|d| !d.as_os_str().is_empty()) {
            std::fs::create_dir_all(d).map_err(|e| e.to_string())?;
        }
        export::write(t, &o, f)?;
        eprintln!("Exportado: {}", o.display());
        done.push(o);
    }
    Ok(done)
}

fn run(cli: Cli) -> Result<(), String> {
    let no_cancel = AtomicBool::new(false);
    let now = chrono::Utc::now().timestamp();
    match cli.cmd {
        Cmd::Varrer {
            pasta,
            scan,
            profundidade,
            arquivos,
            exportar,
            salvar,
            historico,
            top,
            email,
        } => {
            let s = do_scan(&pasta, &scan)?;
            println!("\n{:>12}  {:>7}  Pasta/arquivo", "Tamanho", "%");
            for c in s.root.children.iter().take(20) {
                let pct = if s.root.size > 0 {
                    c.size as f64 / s.root.size as f64
                } else {
                    0.0
                };
                let sep = if c.is_dir {
                    std::path::MAIN_SEPARATOR_STR
                } else {
                    ""
                };
                println!(
                    "{:>12}  {:>7}  {}{}",
                    fmt::size(c.size),
                    fmt::pct(pct),
                    c.name,
                    sep
                );
            }
            if top > 0 {
                println!("\nMaiores arquivos:");
                for f in analysis::largest(&s.root, &s.root_path, top) {
                    println!("{:>12}  {}", fmt::size(f.size), f.path.display());
                }
            }
            if let Some(p) = salvar {
                let p = out_path(&p);
                snapshot::save(&s, &p).map_err(|e| e.to_string())?;
                eprintln!("Instantâneo: {}", p.display());
            }
            if historico {
                let e = snapshot::add_history(&s).map_err(|e| e.to_string())?;
                eprintln!("Histórico: {}", e.file.display());
            }
            let mut t = export::tree_table(&s.root, &s.root_path, profundidade, arquivos);
            t.title = export::summary(&s);
            let files = export_all(&t, &exportar)?;
            if let Some(to) = email.email_para {
                send_mail(
                    &to,
                    email.email_de.as_deref(),
                    email.smtp.as_deref(),
                    email.smtp_usuario.as_deref(),
                    &t.title,
                    &files,
                )?;
            }
        }
        Cmd::Buscar {
            pasta,
            scan,
            nome,
            regex,
            tipo,
            min,
            max,
            antigo_dias,
            recente_dias,
            caminho_min,
            conteudo,
            modelo,
            limite,
            exportar,
        } => {
            let s = do_scan(&pasta, &scan)?;
            let c = match modelo {
                Some(m) => {
                    let text =
                        std::fs::read_to_string(&m).map_err(|e| format!("{}: {e}", m.display()))?;
                    Criteria::from_json(&text)?
                }
                None => Criteria {
                    kind: match tipo {
                        Tipo::Arquivos => Kind::Files,
                        Tipo::Pastas => Kind::Folders,
                        Tipo::Ambos => Kind::Both,
                    },
                    name: nome,
                    regex,
                    min_size: min.as_deref().and_then(fmt::parse_size),
                    max_size: max.as_deref().and_then(fmt::parse_size),
                    older_than_days: antigo_dias,
                    newer_than_days: recente_dias,
                    min_path_len: caminho_min,
                    content: conteudo,
                    ..Default::default()
                },
            };
            let hits = search::search(&s.root, &s.root_path, &c, now, &no_cancel)?;
            let total: u64 = hits.iter().map(|h| h.size).sum();
            println!(
                "{} resultados, {}",
                fmt::int(hits.len() as i64),
                fmt::size(total)
            );
            for h in hits.iter().take(limite) {
                println!(
                    "{:>12}  {}  {}",
                    fmt::size(h.size),
                    fmt::date(h.mtime),
                    h.path.display()
                );
            }
            export_all(
                &search::hits_table(format!("Busca em {}", s.root_path.display()), &hits, false),
                &exportar,
            )?;
        }
        Cmd::Duplicados {
            pasta,
            scan,
            min,
            modo,
            exportar,
        } => {
            let s = do_scan(&pasta, &scan)?;
            let mode = match modo {
                Modo::NomeTamanho => DupMode::NameSize,
                Modo::Md5 => DupMode::Md5,
                Modo::Sha256 => DupMode::Sha256,
            };
            let min = fmt::parse_size(&min).ok_or("tamanho mínimo inválido")?;
            let groups = search::duplicates(
                &s.root,
                &s.root_path,
                min,
                mode,
                &no_cancel,
                &DupProgress::default(),
            );
            let waste: u64 = groups.iter().map(|g| search::wasted(g)).sum();
            println!(
                "{} grupos de duplicados; {} poderiam ser liberados",
                fmt::int(groups.len() as i64),
                fmt::size(waste)
            );
            for g in groups.iter().take(20) {
                println!(
                    "\n{} × {}  (desperdício {})",
                    g.len(),
                    fmt::size(g[0].size),
                    fmt::size(search::wasted(g))
                );
                for h in g {
                    println!("    {}", h.path.display());
                }
            }
            let all: Vec<_> = groups.into_iter().flatten().collect();
            export_all(
                &search::hits_table(
                    format!("Duplicados em {}", s.root_path.display()),
                    &all,
                    true,
                ),
                &exportar,
            )?;
        }
        Cmd::Comparar {
            antigo,
            novo,
            profundidade,
            arquivos,
            limite,
            exportar,
        } => {
            let load = |p: &Path| -> Result<Scan, String> {
                if p.is_dir() {
                    do_scan(p, &ScanArgs::default())
                } else {
                    snapshot::load(p).map_err(|e| format!("{}: {e}", p.display()))
                }
            };
            let (a, b) = (load(&antigo)?, load(&novo)?);
            let diffs = snapshot::compare(&a.root, &b.root, profundidade, arquivos);
            println!(
                "{} ({}) → {} ({}): {}",
                fmt::date(a.when),
                fmt::size(a.root.size),
                fmt::date(b.when),
                fmt::size(b.root.size),
                fmt::signed(b.root.size as i64 - a.root.size as i64)
            );
            for d in diffs.iter().take(limite) {
                println!(
                    "{:>14}  {:>12} → {:<12} {:<9} {}",
                    fmt::signed(d.delta()),
                    fmt::size(d.old_size),
                    fmt::size(d.new_size),
                    d.status.label(),
                    d.path.display()
                );
            }
            let title = format!("Comparação {} → {}", antigo.display(), novo.display());
            export_all(&snapshot::diff_table(title, &diffs), &exportar)?;
        }
        Cmd::Historico { pasta } => {
            let root = atlasdisco_core::scan::normalize(&pasta).map_err(|e| e.to_string())?;
            let h = snapshot::history(&root);
            if h.is_empty() {
                println!(
                    "Sem histórico para {} (use: varrer --historico).",
                    root.display()
                );
            }
            let mut prev: Option<u64> = None;
            for e in &h {
                let d = prev
                    .map(|p| fmt::signed(e.size as i64 - p as i64))
                    .unwrap_or_default();
                println!(
                    "{}  {:>12}  {:>12}  {} arquivos",
                    fmt::date(e.when),
                    fmt::size(e.size),
                    d,
                    fmt::int(e.files as i64)
                );
                prev = Some(e.size);
            }
        }
        Cmd::Agendar {
            nome,
            frequencia,
            hora,
            comando,
        } => {
            let (hh, mm) = hora
                .split_once(':')
                .and_then(|(h, m)| Some((h.parse().ok()?, m.parse().ok()?)))
                .ok_or("hora inválida (use HH:MM)")?;
            if hh > 23 || mm > 59 {
                return Err("hora inválida (use HH:MM)".into());
            }
            let exe = std::env::current_exe().map_err(|e| e.to_string())?;
            let f = match frequencia {
                Freq::Diario => ops::Frequency::Daily,
                Freq::Semanal => ops::Frequency::Weekly,
                Freq::Mensal => ops::Frequency::Monthly,
            };
            println!("{}", ops::schedule(&nome, &exe, &comando, f, hh, mm)?);
        }
        Cmd::Desagendar { nome } => println!("{}", ops::unschedule(&nome)?),
    }
    Ok(())
}

fn send_mail(
    to: &str,
    from: Option<&str>,
    smtp: Option<&str>,
    user: Option<&str>,
    subject: &str,
    files: &[PathBuf],
) -> Result<(), String> {
    use lettre::message::{header::ContentType, Attachment, MultiPart, SinglePart};
    use lettre::transport::smtp::authentication::Credentials;
    use lettre::{Message, SmtpTransport, Transport};
    let smtp = smtp.ok_or("informe --smtp servidor[:porta]")?;
    let from = from
        .or(user)
        .ok_or("informe --email-de ou --smtp-usuario")?;
    let mut mp = MultiPart::mixed().singlepart(SinglePart::plain(format!(
        "{subject}\n\nRelatório gerado pelo AtlasDisco."
    )));
    for f in files {
        let body = std::fs::read(f).map_err(|e| e.to_string())?;
        let name = f
            .file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .into_owned();
        mp = mp.singlepart(Attachment::new(name).body(
            body,
            ContentType::parse("application/octet-stream").unwrap(),
        ));
    }
    let msg = Message::builder()
        .from(from.parse().map_err(|e| format!("remetente: {e}"))?)
        .to(to.parse().map_err(|e| format!("destinatário: {e}"))?)
        .subject(subject)
        .multipart(mp)
        .map_err(|e| e.to_string())?;
    let (host, port) = match smtp.split_once(':') {
        Some((h, p)) => (h, p.parse::<u16>().map_err(|_| "porta SMTP inválida")?),
        None => (smtp, 587),
    };
    let builder = if port == 465 {
        SmtpTransport::relay(host)
    } else {
        SmtpTransport::starttls_relay(host)
    }
    .map_err(|e| e.to_string())?;
    let mut builder = builder.port(port);
    if let Some(u) = user {
        let pass = std::env::var("ATLASDISCO_SMTP_SENHA")
            .map_err(|_| "defina a senha na variável ATLASDISCO_SMTP_SENHA")?;
        builder = builder.credentials(Credentials::new(u.into(), pass));
    }
    builder.build().send(&msg).map_err(|e| e.to_string())?;
    eprintln!("E-mail enviado para {to}");
    Ok(())
}
