//! Fontes remotas: Linux/Unix via SSH, Amazon S3 (e compatíveis), Azure Blob Storage e WebDAV.
//! Todas viram uma lista plana de entradas que é montada na mesma árvore das varreduras locais.

use crate::model::{Node, Scan};
use crate::scan::Progress;
use hmac::{Hmac, Mac};
use regex::Regex;
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::Ordering::Relaxed;
use std::time::Instant;

/// Uma entrada da listagem remota. `path` é relativo à raiz, separado por '/'.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Entry {
    pub path: String,
    pub is_dir: bool,
    pub size: u64,
    pub alloc: u64,
    pub mtime: i64,
    pub atime: i64,
}

/// Monta a árvore (cria as pastas intermediárias que não vieram na listagem) e calcula os totais.
pub fn build_tree(root_name: &str, entries: &[Entry]) -> Node {
    let mut root = Node {
        name: root_name.to_string(),
        is_dir: true,
        ..Default::default()
    };
    // índice: caminho da pasta → posição na árvore
    let mut index: HashMap<String, Vec<usize>> = HashMap::new();
    index.insert(String::new(), vec![]);
    fn ensure_dir(
        root: &mut Node,
        index: &mut HashMap<String, Vec<usize>>,
        path: &str,
    ) -> Vec<usize> {
        if let Some(i) = index.get(path) {
            return i.clone();
        }
        let (parent, name) = match path.rsplit_once('/') {
            Some((p, n)) => (p.to_string(), n.to_string()),
            None => (String::new(), path.to_string()),
        };
        let mut pidx = ensure_dir(root, index, &parent);
        let p = root.get_mut(&pidx).unwrap();
        p.children.push(Node {
            name,
            is_dir: true,
            ..Default::default()
        });
        pidx.push(p.children.len() - 1);
        index.insert(path.to_string(), pidx.clone());
        pidx
    }
    for e in entries {
        let path = e.path.trim_matches('/');
        if path.is_empty() {
            if e.is_dir {
                root.mtime = e.mtime;
            }
            continue;
        }
        if e.is_dir {
            let idx = ensure_dir(&mut root, &mut index, path);
            let n = root.get_mut(&idx).unwrap();
            n.mtime = n.mtime.max(e.mtime);
            n.atime = e.atime;
            continue;
        }
        let (parent, name) = match path.rsplit_once('/') {
            Some((p, n)) => (p, n),
            None => ("", path),
        };
        let pidx = ensure_dir(&mut root, &mut index, parent);
        root.get_mut(&pidx).unwrap().children.push(Node {
            name: name.to_string(),
            size: e.size,
            alloc: e.alloc,
            mtime: e.mtime,
            atime: e.atime,
            ..Default::default()
        });
    }
    root.recompute();
    root.sort_by_size();
    root
}

fn make_scan(root_path: String, root: Node, started: Instant, errors: Vec<String>) -> Scan {
    Scan {
        format: Scan::FORMAT,
        root_path: PathBuf::from(&root_path),
        when: chrono::Utc::now().timestamp(),
        seconds: started.elapsed().as_secs_f64(),
        cluster: 0,
        volume_total: 0,
        volume_free: 0,
        folders_only: false,
        errors,
        root,
    }
}

fn cancelled(p: &Progress) -> Result<(), String> {
    if p.cancel.load(Relaxed) {
        Err("cancelado".into())
    } else {
        Ok(())
    }
}

// ------------------------------------------------------------------ SSH

/// Varre `path` em `destino` (usuario@host) com o cliente `ssh` do sistema e `find -printf` (GNU) do lado remoto.
/// A autenticação é a do próprio ssh (chave/agente); não pede senha (BatchMode).
pub fn scan_ssh(target: &str, path: &str, port: Option<u16>, p: &Progress) -> Result<Scan, String> {
    let started = Instant::now();
    if target.is_empty() || target.starts_with('-') || target.contains(char::is_whitespace) {
        return Err("destino SSH inválido (use usuario@servidor)".into());
    }
    let path = if path.trim().is_empty() {
        "/"
    } else {
        path.trim()
    };
    let remote = format!(
        "LC_ALL=C find {} -xdev -printf '%y\\t%s\\t%b\\t%T@\\t%A@\\t%P\\0'",
        shell_quote(path)
    );
    let mut cmd = std::process::Command::new("ssh");
    cmd.args(["-o", "BatchMode=yes", "-o", "ConnectTimeout=15"]);
    if let Some(port) = port {
        cmd.args(["-p", &port.to_string()]);
    }
    cmd.arg("--").arg(target).arg(remote);
    *p.current.lock().unwrap() = format!("ssh {target} …");
    let out = cmd
        .output()
        .map_err(|e| format!("não foi possível executar o ssh: {e}"))?;
    cancelled(p)?;
    let stderr = String::from_utf8_lossy(&out.stderr).into_owned();
    if out.stdout.is_empty() && !out.status.success() {
        return Err(format!("ssh falhou: {}", stderr.trim()));
    }
    let entries = parse_find(&out.stdout);
    p.files
        .store(entries.iter().filter(|e| !e.is_dir).count() as u64, Relaxed);
    let root = build_tree(path, &entries);
    // find devolve erros de permissão em stderr, um por linha.
    let errors = stderr
        .lines()
        .filter(|l| !l.trim().is_empty())
        .map(String::from)
        .collect();
    Ok(make_scan(
        format!(
            "ssh://{target}{}{path}",
            if path.starts_with('/') { "" } else { "/" }
        ),
        root,
        started,
        errors,
    ))
}

fn shell_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', "'\\''"))
}

/// Saída de `find -printf '%y\t%s\t%b\t%T@\t%A@\t%P\0'`.
pub fn parse_find(out: &[u8]) -> Vec<Entry> {
    out.split(|&b| b == 0)
        .filter(|r| !r.is_empty())
        .filter_map(|r| {
            let s = String::from_utf8_lossy(r);
            let mut f = s.splitn(6, '\t');
            let kind = f.next()?.trim_start_matches('\n');
            let size: u64 = f.next()?.parse().ok()?;
            let blocks: u64 = f.next()?.parse().ok()?;
            let t = |v: Option<&str>| {
                v.and_then(|x| x.parse::<f64>().ok())
                    .map(|x| x as i64)
                    .unwrap_or(0)
            };
            let mtime = t(f.next());
            let atime = t(f.next());
            let path = f.next()?.to_string();
            match kind {
                "d" => Some(Entry {
                    path,
                    is_dir: true,
                    mtime,
                    atime,
                    ..Default::default()
                }),
                "f" => Some(Entry {
                    path,
                    is_dir: false,
                    size,
                    alloc: blocks * 512,
                    mtime,
                    atime,
                }),
                // links, sockets etc.: só o próprio inode
                _ => Some(Entry {
                    path,
                    is_dir: false,
                    size: 0,
                    alloc: 0,
                    mtime,
                    atime,
                }),
            }
        })
        .collect()
}

// ------------------------------------------------------------------ Amazon S3 (assinatura v4)

pub struct S3 {
    pub bucket: String,
    pub prefix: String,
    pub region: String,
    pub access_key: String,
    pub secret_key: String,
    pub session_token: Option<String>,
    /// Endpoint compatível (MinIO, Wasabi, R2...). Vazio = AWS. Com endpoint, usa endereçamento por caminho.
    pub endpoint: String,
}

type HmacSha256 = Hmac<Sha256>;

fn hmac(key: &[u8], data: &str) -> Vec<u8> {
    let mut m = HmacSha256::new_from_slice(key).expect("hmac aceita qualquer chave");
    m.update(data.as_bytes());
    m.finalize().into_bytes().to_vec()
}

fn hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

/// Codificação de URI da assinatura v4 (RFC 3986; '/' preservada só no caminho).
pub fn uri_encode(s: &str, keep_slash: bool) -> String {
    let mut o = String::new();
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                o.push(b as char)
            }
            b'/' if keep_slash => o.push('/'),
            _ => o.push_str(&format!("%{b:02X}")),
        }
    }
    o
}

/// Assinatura AWS v4. `headers` em minúsculas; devolve o valor do cabeçalho Authorization.
#[allow(clippy::too_many_arguments)]
pub fn sigv4(
    method: &str,
    path: &str,
    query: &[(String, String)],
    headers: &[(String, String)],
    payload_hash: &str,
    amz_date: &str,
    region: &str,
    service: &str,
    access: &str,
    secret: &str,
) -> String {
    let mut q: Vec<(String, String)> = query
        .iter()
        .map(|(k, v)| (uri_encode(k, false), uri_encode(v, false)))
        .collect();
    q.sort();
    let cq = q
        .iter()
        .map(|(k, v)| format!("{k}={v}"))
        .collect::<Vec<_>>()
        .join("&");
    let mut h: Vec<(String, String)> = headers
        .iter()
        .map(|(k, v)| (k.to_lowercase(), v.trim().to_string()))
        .collect();
    h.sort();
    let ch: String = h.iter().map(|(k, v)| format!("{k}:{v}\n")).collect();
    let signed = h
        .iter()
        .map(|(k, _)| k.as_str())
        .collect::<Vec<_>>()
        .join(";");
    let creq = format!(
        "{method}\n{}\n{cq}\n{ch}\n{signed}\n{payload_hash}",
        uri_encode(path, true)
    );
    let date = &amz_date[..8];
    let scope = format!("{date}/{region}/{service}/aws4_request");
    let sts = format!(
        "AWS4-HMAC-SHA256\n{amz_date}\n{scope}\n{}",
        hex(&Sha256::digest(creq.as_bytes()))
    );
    let k = hmac(format!("AWS4{secret}").as_bytes(), date);
    let k = hmac(&k, region);
    let k = hmac(&k, service);
    let k = hmac(&k, "aws4_request");
    let sig = hex(&hmac(&k, &sts));
    format!("AWS4-HMAC-SHA256 Credential={access}/{scope}, SignedHeaders={signed}, Signature={sig}")
}

pub fn scan_s3(c: &S3, p: &Progress) -> Result<Scan, String> {
    let started = Instant::now();
    let empty_hash = hex(&Sha256::digest(b""));
    let (host, base_path, scheme) = if c.endpoint.trim().is_empty() {
        (
            format!("{}.s3.{}.amazonaws.com", c.bucket, c.region),
            "/".to_string(),
            "https",
        )
    } else {
        let ep = c.endpoint.trim().trim_end_matches('/');
        let (scheme, host) = ep.split_once("://").unwrap_or(("https", ep));
        (
            host.to_string(),
            format!("/{}", c.bucket),
            if scheme == "http" { "http" } else { "https" },
        )
    };
    let mut entries = Vec::new();
    let mut token: Option<String> = None;
    loop {
        cancelled(p)?;
        let mut query = vec![
            ("list-type".to_string(), "2".to_string()),
            ("max-keys".to_string(), "1000".to_string()),
        ];
        if !c.prefix.is_empty() {
            query.push(("prefix".into(), c.prefix.clone()));
        }
        if let Some(t) = &token {
            query.push(("continuation-token".into(), t.clone()));
        }
        let amz_date = chrono::Utc::now().format("%Y%m%dT%H%M%SZ").to_string();
        let mut headers = vec![
            ("host".to_string(), host.clone()),
            ("x-amz-content-sha256".to_string(), empty_hash.clone()),
            ("x-amz-date".to_string(), amz_date.clone()),
        ];
        if let Some(t) = &c.session_token {
            headers.push(("x-amz-security-token".into(), t.clone()));
        }
        let auth = sigv4(
            "GET",
            &base_path,
            &query,
            &headers,
            &empty_hash,
            &amz_date,
            &c.region,
            "s3",
            &c.access_key,
            &c.secret_key,
        );
        let qs = query
            .iter()
            .map(|(k, v)| format!("{}={}", uri_encode(k, false), uri_encode(v, false)))
            .collect::<Vec<_>>()
            .join("&");
        let url = format!("{scheme}://{host}{}?{qs}", uri_encode(&base_path, true));
        let mut req = ureq::get(&url).set("Authorization", &auth);
        for (k, v) in &headers {
            if k != "host" {
                req = req.set(k, v);
            }
        }
        let body = http_text(req.call())?;
        let (page, next) = parse_s3_list(&body, &c.prefix);
        p.files.fetch_add(page.len() as u64, Relaxed);
        p.bytes
            .fetch_add(page.iter().map(|e| e.size).sum(), Relaxed);
        entries.extend(page);
        *p.current.lock().unwrap() =
            format!("s3://{}/{} — {} objetos", c.bucket, c.prefix, entries.len());
        match next {
            Some(t) => token = Some(t),
            None => break,
        }
    }
    let root_name = format!("s3://{}/{}", c.bucket, c.prefix);
    let root = build_tree(&root_name, &entries);
    Ok(make_scan(root_name, root, started, vec![]))
}

fn xml_tag<'a>(s: &'a str, tag: &str) -> Option<&'a str> {
    let open = format!("<{tag}>");
    let i = s.find(&open)? + open.len();
    let j = s[i..].find(&format!("</{tag}>"))? + i;
    Some(&s[i..j])
}

fn xml_unescape(s: &str) -> String {
    s.replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&apos;", "'")
        .replace("&amp;", "&")
}

fn iso_time(s: &str) -> i64 {
    chrono::DateTime::parse_from_rfc3339(s.trim())
        .or_else(|_| chrono::DateTime::parse_from_rfc2822(s.trim()))
        .map(|d| d.timestamp())
        .unwrap_or(0)
}

/// Uma página de ListObjectsV2 → entradas e token da próxima página.
pub fn parse_s3_list(xml: &str, prefix: &str) -> (Vec<Entry>, Option<String>) {
    let mut v = Vec::new();
    for part in xml.split("<Contents>").skip(1) {
        let key = xml_unescape(xml_tag(part, "Key").unwrap_or(""));
        let rel = key.strip_prefix(prefix).unwrap_or(&key).to_string();
        let size = xml_tag(part, "Size")
            .and_then(|s| s.trim().parse().ok())
            .unwrap_or(0);
        let mtime = xml_tag(part, "LastModified").map(iso_time).unwrap_or(0);
        if rel.ends_with('/') {
            v.push(Entry {
                path: rel,
                is_dir: true,
                mtime,
                ..Default::default()
            });
        } else {
            v.push(Entry {
                path: rel,
                size,
                alloc: size,
                mtime,
                ..Default::default()
            });
        }
    }
    let truncated = xml_tag(xml, "IsTruncated")
        .map(|s| s.trim() == "true")
        .unwrap_or(false);
    let next = if truncated {
        xml_tag(xml, "NextContinuationToken").map(xml_unescape)
    } else {
        None
    };
    (v, next)
}

// ------------------------------------------------------------------ Azure Blob (URL SAS do contêiner)

/// `sas_url`: https://conta.blob.core.windows.net/conteiner?sv=...&sig=... (permissão de listagem).
pub fn scan_azure(sas_url: &str, prefix: &str, p: &Progress) -> Result<Scan, String> {
    let started = Instant::now();
    let (base, sas) = sas_url
        .trim()
        .split_once('?')
        .ok_or("informe a URL SAS completa do contêiner (com ?sv=...)")?;
    let mut entries = Vec::new();
    let mut marker = String::new();
    loop {
        cancelled(p)?;
        let mut url = format!("{base}?restype=container&comp=list&maxresults=5000&{sas}");
        if !prefix.is_empty() {
            url += &format!("&prefix={}", uri_encode(prefix, false));
        }
        if !marker.is_empty() {
            url += &format!("&marker={}", uri_encode(&marker, false));
        }
        let body = http_text(ureq::get(&url).set("x-ms-version", "2021-08-06").call())?;
        let (page, next) = parse_azure_list(&body, prefix);
        p.files.fetch_add(page.len() as u64, Relaxed);
        entries.extend(page);
        *p.current.lock().unwrap() = format!("{base} — {} blobs", entries.len());
        match next {
            Some(m) if !m.is_empty() => marker = m,
            _ => break,
        }
    }
    let root_name = format!("{}/{}", base.trim_end_matches('/'), prefix);
    let root = build_tree(&root_name, &entries);
    Ok(make_scan(root_name, root, started, vec![]))
}

pub fn parse_azure_list(xml: &str, prefix: &str) -> (Vec<Entry>, Option<String>) {
    let mut v = Vec::new();
    for part in xml.split("<Blob>").skip(1) {
        let name = xml_unescape(xml_tag(part, "Name").unwrap_or(""));
        let rel = name.strip_prefix(prefix).unwrap_or(&name).to_string();
        let size = xml_tag(part, "Content-Length")
            .and_then(|s| s.trim().parse().ok())
            .unwrap_or(0);
        let mtime = xml_tag(part, "Last-Modified").map(iso_time).unwrap_or(0);
        v.push(Entry {
            path: rel,
            size,
            alloc: size,
            mtime,
            ..Default::default()
        });
    }
    (v, xml_tag(xml, "NextMarker").map(xml_unescape))
}

// ------------------------------------------------------------------ WebDAV (Nextcloud, ownCloud, NAS...)

pub fn scan_webdav(url: &str, user: &str, pass: &str, p: &Progress) -> Result<Scan, String> {
    let started = Instant::now();
    let url = url.trim().trim_end_matches('/').to_string() + "/";
    let auth = if user.is_empty() {
        None
    } else {
        use base64::Engine;
        Some(format!(
            "Basic {}",
            base64::engine::general_purpose::STANDARD.encode(format!("{user}:{pass}"))
        ))
    };
    let base_path = url_path(&url);
    let origin = url[..url.len() - base_path.len()].to_string();
    let body = r#"<?xml version="1.0"?><d:propfind xmlns:d="DAV:"><d:prop><d:resourcetype/><d:getcontentlength/><d:getlastmodified/></d:prop></d:propfind>"#;
    let mut entries = Vec::new();
    let mut errors = Vec::new();
    let mut queue = vec![base_path.clone()];
    let mut seen = std::collections::HashSet::new();
    while let Some(dir) = queue.pop() {
        cancelled(p)?;
        if !seen.insert(dir.clone()) {
            continue;
        }
        *p.current.lock().unwrap() = format!("{origin}{dir}");
        let mut req = ureq::request("PROPFIND", &format!("{origin}{dir}"))
            .set("Depth", "1")
            .set("Content-Type", "application/xml");
        if let Some(a) = &auth {
            req = req.set("Authorization", a);
        }
        let xml = match http_text(req.send_string(body)) {
            Ok(x) => x,
            Err(e) if dir != base_path => {
                errors.push(format!("{dir} — {e}"));
                continue;
            }
            Err(e) => return Err(e),
        };
        for e in parse_webdav(&xml) {
            let href = percent_decode(&url_path(&e.path));
            let base_dec = percent_decode(&base_path);
            let Some(rel) = href.strip_prefix(&base_dec) else {
                continue;
            };
            let rel = rel.trim_end_matches('/').to_string();
            if rel.is_empty()
                || href.trim_end_matches('/') == percent_decode(&dir).trim_end_matches('/')
            {
                continue;
            }
            if e.is_dir {
                queue.push(url_path(&e.path));
            } else {
                p.files.fetch_add(1, Relaxed);
                p.bytes.fetch_add(e.size, Relaxed);
            }
            entries.push(Entry { path: rel, ..e });
        }
    }
    let root = build_tree(&url, &entries);
    Ok(make_scan(url, root, started, errors))
}

/// Caminho de uma URL ("/remote.php/dav/files/ana/") — aceita href absoluto ou relativo.
fn url_path(u: &str) -> String {
    match u.split_once("://") {
        Some((_, rest)) => rest
            .find('/')
            .map(|i| rest[i..].to_string())
            .unwrap_or_else(|| "/".into()),
        None => u.to_string(),
    }
}

fn percent_decode(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%' && i + 2 < b.len() {
            if let Ok(v) = u8::from_str_radix(&s[i + 1..i + 3], 16) {
                out.push(v);
                i += 3;
                continue;
            }
        }
        out.push(b[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// Resposta multistatus do PROPFIND. Em cada `Entry`, `path` traz o href como veio.
pub fn parse_webdav(xml: &str) -> Vec<Entry> {
    let resp = Regex::new(r"(?is)<(?:[a-z0-9]+:)?response\b.*?</(?:[a-z0-9]+:)?response>").unwrap();
    let tag = |name: &str| {
        Regex::new(&format!(
            r"(?is)<(?:[a-z0-9]+:)?{name}\b[^>]*>(.*?)</(?:[a-z0-9]+:)?{name}>"
        ))
        .unwrap()
    };
    let (href, len, modified) = (tag("href"), tag("getcontentlength"), tag("getlastmodified"));
    let coll = Regex::new(r"(?is)<(?:[a-z0-9]+:)?collection\s*/?>").unwrap();
    resp.find_iter(xml)
        .filter_map(|m| {
            let r = m.as_str();
            let h = xml_unescape(href.captures(r)?.get(1)?.as_str().trim());
            let is_dir = coll.is_match(r);
            let size = len
                .captures(r)
                .and_then(|c| c[1].trim().parse().ok())
                .unwrap_or(0);
            let mtime = modified.captures(r).map(|c| iso_time(&c[1])).unwrap_or(0);
            Some(Entry {
                path: h,
                is_dir,
                size: if is_dir { 0 } else { size },
                alloc: if is_dir { 0 } else { size },
                mtime,
                atime: 0,
            })
        })
        .collect()
}

fn http_text(r: Result<ureq::Response, ureq::Error>) -> Result<String, String> {
    match r {
        Ok(resp) => resp.into_string().map_err(|e| e.to_string()),
        Err(ureq::Error::Status(code, resp)) => {
            let body = resp.into_string().unwrap_or_default();
            let msg = xml_tag(&body, "Message")
                .or_else(|| xml_tag(&body, "Code"))
                .unwrap_or("")
                .to_string();
            Err(format!("HTTP {code} {msg}").trim().to_string())
        }
        Err(e) => Err(e.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn arvore_a_partir_de_lista() {
        let e = vec![
            Entry {
                path: "a/b/c.txt".into(),
                size: 10,
                alloc: 10,
                ..Default::default()
            },
            Entry {
                path: "a/d.bin".into(),
                size: 5,
                alloc: 5,
                ..Default::default()
            },
            Entry {
                path: "vazia".into(),
                is_dir: true,
                ..Default::default()
            },
            Entry {
                path: "x".into(),
                size: 1,
                alloc: 1,
                ..Default::default()
            },
        ];
        let r = build_tree("raiz", &e);
        assert_eq!(r.size, 16);
        assert_eq!(r.files, 3);
        assert_eq!(r.dirs, 3); // a, a/b, vazia
        assert_eq!(r.children[0].name, "a");
        assert_eq!(r.children[0].size, 15);
    }

    #[test]
    fn saida_do_find() {
        let out = b"d\t4096\t8\t1700000000.5\t1700000001.0\t\0f\t1234\t8\t1700000000.0\t1700000000.0\tdocs/nota com espa\xc3\xa7o.txt\0d\t4096\t8\t1.0\t1.0\tdocs\0l\t7\t0\t1.0\t1.0\tlink\0";
        let e = parse_find(out);
        assert_eq!(e.len(), 4);
        assert!(e[0].is_dir && e[0].path.is_empty());
        assert_eq!(e[1].path, "docs/nota com espaço.txt");
        assert_eq!(e[1].alloc, 4096);
        let r = build_tree("/srv", &e);
        assert_eq!(r.size, 1234);
    }

    #[test]
    fn listagem_s3_e_azure() {
        let s3 = "<ListBucketResult><IsTruncated>true</IsTruncated><Contents><Key>fotos/a&amp;b.jpg</Key><LastModified>2024-05-01T10:00:00.000Z</LastModified><Size>500</Size></Contents><Contents><Key>fotos/sub/</Key><Size>0</Size></Contents><NextContinuationToken>tok/1=</NextContinuationToken></ListBucketResult>";
        let (v, next) = parse_s3_list(s3, "fotos/");
        assert_eq!(v[0].path, "a&b.jpg");
        assert_eq!(v[0].size, 500);
        assert!(v[0].mtime > 0);
        assert!(v[1].is_dir);
        assert_eq!(next.as_deref(), Some("tok/1="));
        let az = "<EnumerationResults><Blobs><Blob><Name>x/y.bin</Name><Properties><Last-Modified>Wed, 01 May 2024 10:00:00 GMT</Last-Modified><Content-Length>42</Content-Length></Properties></Blob></Blobs><NextMarker /></EnumerationResults>";
        let (v, next) = parse_azure_list(az, "");
        assert_eq!(v[0].path, "x/y.bin");
        assert_eq!(v[0].size, 42);
        assert!(v[0].mtime > 0);
        assert!(next.is_none());
    }

    #[test]
    fn propfind() {
        let xml = r#"<?xml version="1.0"?><d:multistatus xmlns:d="DAV:"><d:response><d:href>/dav/ana/</d:href><d:propstat><d:prop><d:resourcetype><d:collection/></d:resourcetype></d:prop></d:propstat></d:response><d:response><d:href>/dav/ana/F%C3%A9rias.mp4</d:href><d:propstat><d:prop><d:resourcetype/><d:getcontentlength>9000</d:getcontentlength><d:getlastmodified>Wed, 01 May 2024 10:00:00 GMT</d:getlastmodified></d:prop></d:propstat></d:response></d:multistatus>"#;
        let e = parse_webdav(xml);
        assert_eq!(e.len(), 2);
        assert!(e[0].is_dir);
        assert_eq!(e[1].size, 9000);
        assert_eq!(percent_decode(&e[1].path), "/dav/ana/Férias.mp4");
    }

    /// Exemplos oficiais da documentação da AWS (Signature Version 4, "examplebucket").
    #[test]
    fn assinatura_v4_exemplos_aws() {
        let secret = "wJalrXUtnFEMI/K7MDENG/bPxRfiCYEXAMPLEKEY";
        let empty = "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855";
        let h = |v: &[(&str, &str)]| {
            v.iter()
                .map(|(a, b)| (a.to_string(), b.to_string()))
                .collect::<Vec<_>>()
        };
        // GET Object com Range
        let auth = sigv4(
            "GET",
            "/test.txt",
            &[],
            &h(&[
                ("host", "examplebucket.s3.amazonaws.com"),
                ("range", "bytes=0-9"),
                ("x-amz-content-sha256", empty),
                ("x-amz-date", "20130524T000000Z"),
            ]),
            empty,
            "20130524T000000Z",
            "us-east-1",
            "s3",
            "AKIAIOSFODNN7EXAMPLE",
            secret,
        );
        assert!(
            auth.ends_with(
                "Signature=f0e8bdb87c964420e857bd35b5d6ed310bd44f0170aba48dd91039c6036bdb41"
            ),
            "{auth}"
        );
        // GET Bucket (List Objects)
        let auth = sigv4(
            "GET",
            "/",
            &[
                ("max-keys".into(), "2".into()),
                ("prefix".into(), "J".into()),
            ],
            &h(&[
                ("host", "examplebucket.s3.amazonaws.com"),
                ("x-amz-content-sha256", empty),
                ("x-amz-date", "20130524T000000Z"),
            ]),
            empty,
            "20130524T000000Z",
            "us-east-1",
            "s3",
            "AKIAIOSFODNN7EXAMPLE",
            secret,
        );
        assert!(
            auth.ends_with(
                "Signature=34b48302e7b5fa45bde8084f4b7868a86f0a534bc59db6670ed5711ef69dc6f7"
            ),
            "{auth}"
        );
    }
}
