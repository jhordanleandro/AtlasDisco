//! Detalhes específicos de cada sistema: espaço alocado, cluster, hardlinks, proprietário e volumes.

use std::fs::Metadata;
use std::path::Path;

/// Uma entrada da lista de controle de acesso (ACL) de um arquivo ou pasta.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Ace {
    pub account: String,
    /// `false` = negação.
    pub allow: bool,
    pub mask: u32,
    pub inherited: bool,
}

impl Ace {
    /// Formato compacto: "+R +W -X".
    pub fn compact(&self) -> String {
        let (r, w, x) = rwx(self.mask);
        let s = if self.allow { '+' } else { '-' };
        [(r, 'R'), (w, 'W'), (x, 'X')]
            .iter()
            .filter(|(on, _)| *on)
            .map(|(_, c)| format!("{s}{c}"))
            .collect::<Vec<_>>()
            .join(" ")
    }

    /// Nome do nível de acesso no vocabulário do Windows.
    pub fn level(&self) -> &'static str {
        let m = self.mask;
        if m & 0x1000_0000 != 0 || m & 0x1F01FF == 0x1F01FF {
            "Controle total"
        } else if m & 0x1301BF == 0x1301BF {
            "Modificar"
        } else if m & 0x1200A9 == 0x1200A9 {
            "Leitura e execução"
        } else if m & 0x120089 == 0x120089 || m & 0x8000_0000 != 0 {
            "Leitura"
        } else if m & 0x100116 == 0x100116 || m & 0x4000_0000 != 0 {
            "Gravação"
        } else {
            "Especial"
        }
    }
}

/// (leitura, gravação, execução/listagem) a partir da máscara de acesso do Windows.
pub fn rwx(m: u32) -> (bool, bool, bool) {
    let all = m & 0x1000_0000 != 0; // GENERIC_ALL
    (
        all || m & 0x8000_0001 != 0,
        all || m & 0x4000_0002 != 0,
        all || m & 0x2000_0020 != 0,
    )
}

/// Permissões resumidas: "Usuário: +R +W +X; Outro: +R".
pub fn acl_compact(path: &Path) -> String {
    let mut merged: Vec<(String, bool, u32)> = Vec::new();
    for a in acl(path).unwrap_or_default() {
        match merged
            .iter_mut()
            .find(|m| m.0 == a.account && m.1 == a.allow)
        {
            Some(m) => m.2 |= a.mask,
            None => merged.push((a.account, a.allow, a.mask)),
        }
    }
    merged
        .into_iter()
        .map(|(acc, allow, mask)| {
            format!(
                "{acc}: {}",
                Ace {
                    account: String::new(),
                    allow,
                    mask,
                    inherited: false
                }
                .compact()
            )
        })
        .collect::<Vec<_>>()
        .join("; ")
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LocKind {
    Disk,
    Removable,
    Network,
    Optical,
    Cloud,
}

impl LocKind {
    pub fn label(self) -> &'static str {
        match self {
            LocKind::Disk => "Disco local",
            LocKind::Removable => "Removível",
            LocKind::Network => "Rede",
            LocKind::Optical => "Mídia óptica",
            LocKind::Cloud => "Nuvem",
        }
    }
}

/// Unidade ou pasta de nuvem que pode ser varrida.
#[derive(Clone, Debug)]
pub struct Location {
    pub path: String,
    pub label: String,
    pub kind: LocKind,
}

/// Pastas sincronizadas com serviços de nuvem (OneDrive, Dropbox, iCloud, Google Drive).
pub fn cloud_folders() -> Vec<Location> {
    let mut v: Vec<Location> = Vec::new();
    let mut add = |p: std::path::PathBuf, label: &str| {
        if p.is_dir() && !v.iter().any(|l| Path::new(&l.path) == p) {
            v.push(Location {
                path: p.to_string_lossy().into_owned(),
                label: label.into(),
                kind: LocKind::Cloud,
            });
        }
    };
    for (var, label) in [
        ("OneDrive", "OneDrive"),
        ("OneDriveConsumer", "OneDrive pessoal"),
        ("OneDriveCommercial", "OneDrive corporativo"),
    ] {
        if let Some(p) = std::env::var_os(var) {
            add(p.into(), label);
        }
    }
    if let Some(home) = dirs::home_dir() {
        for (sub, label) in [
            ("Dropbox", "Dropbox"),
            ("iCloudDrive", "iCloud Drive"),
            ("Google Drive", "Google Drive"),
            ("OneDrive", "OneDrive"),
        ] {
            add(home.join(sub), label);
        }
        // macOS: provedores de arquivos em ~/Library/CloudStorage
        if let Ok(rd) = std::fs::read_dir(home.join("Library").join("CloudStorage")) {
            for e in rd.flatten() {
                let name = e.file_name().to_string_lossy().into_owned();
                add(e.path(), &name);
            }
        }
    }
    v
}

/// Discos e unidades, seguidos das pastas de nuvem.
pub fn locations() -> Vec<Location> {
    let mut v = imp::volumes();
    let clouds = cloud_folders();
    v.extend(clouds);
    v
}

/// Identidade física de um arquivo (volume, índice) e o número de hardlinks.
pub struct FileId {
    pub volume: u64,
    pub index: u64,
    pub links: u64,
}

#[cfg(windows)]
mod imp {
    use super::FileId;
    use crate::model::*;
    use std::ffi::OsStr;
    use std::fs::Metadata;
    use std::os::windows::ffi::OsStrExt;
    use std::os::windows::fs::MetadataExt;
    use std::path::Path;
    use windows_sys::Win32::Foundation::*;
    use windows_sys::Win32::Security::Authorization::*;
    use windows_sys::Win32::Security::*;
    use windows_sys::Win32::Storage::FileSystem::*;

    pub fn wide(s: &OsStr) -> Vec<u16> {
        s.encode_wide().chain(std::iter::once(0)).collect()
    }

    /// Caminho com prefixo \\?\ para passar do limite de 260 caracteres.
    pub fn long(p: &Path) -> Vec<u16> {
        let s = p.as_os_str().to_string_lossy();
        let s = if s.starts_with(r"\\?\") {
            s.to_string()
        } else if let Some(rest) = s.strip_prefix(r"\\") {
            format!(r"\\?\UNC\{rest}")
        } else {
            format!(r"\\?\{s}")
        };
        wide(OsStr::new(&s))
    }

    pub fn attrs(md: &Metadata) -> u32 {
        md.file_attributes()
    }

    pub fn times(md: &Metadata) -> (i64, i64, i64) {
        // FILETIME (100 ns desde 1601) → segundos Unix
        let c = |ft: u64| {
            if ft == 0 {
                0
            } else {
                (ft / 10_000_000) as i64 - 11_644_473_600
            }
        };
        (
            c(md.last_write_time()),
            c(md.last_access_time()),
            c(md.creation_time()),
        )
    }

    pub fn allocated(path: &Path, md: &Metadata, cluster: u64) -> u64 {
        let mut size = md.file_size();
        if md.file_attributes() & (ATTR_COMPRESSED | ATTR_SPARSE) != 0 {
            let w = long(path);
            let mut hi = 0u32;
            let lo = unsafe { GetCompressedFileSizeW(w.as_ptr(), &mut hi) };
            if lo != INVALID_FILE_SIZE || unsafe { GetLastError() } == 0 {
                size = ((hi as u64) << 32) | lo as u64;
            }
        }
        size.div_ceil(cluster) * cluster
    }

    pub fn volume_root(path: &Path) -> Vec<u16> {
        let w = wide(path.as_os_str());
        let mut buf = vec![0u16; 1024];
        if unsafe { GetVolumePathNameW(w.as_ptr(), buf.as_mut_ptr(), buf.len() as u32) } != 0 {
            let n = buf.iter().position(|&c| c == 0).unwrap_or(0);
            buf.truncate(n + 1);
            buf
        } else {
            w
        }
    }

    pub fn cluster_size(path: &Path) -> u64 {
        let root = volume_root(path);
        let (mut spc, mut bps, mut fc, mut tc) = (0u32, 0u32, 0u32, 0u32);
        if unsafe { GetDiskFreeSpaceW(root.as_ptr(), &mut spc, &mut bps, &mut fc, &mut tc) } != 0
            && spc * bps > 0
        {
            (spc * bps) as u64
        } else {
            4096
        }
    }

    pub fn volume_space(path: &Path) -> (u64, u64) {
        let root = volume_root(path);
        let (mut avail, mut total, mut free) = (0u64, 0u64, 0u64);
        if unsafe { GetDiskFreeSpaceExW(root.as_ptr(), &mut avail, &mut total, &mut free) } != 0 {
            (total, free)
        } else {
            (0, 0)
        }
    }

    pub fn file_id(path: &Path) -> Option<FileId> {
        let w = long(path);
        unsafe {
            let h = CreateFileW(
                w.as_ptr(),
                FILE_READ_ATTRIBUTES,
                FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
                std::ptr::null(),
                OPEN_EXISTING,
                FILE_FLAG_BACKUP_SEMANTICS,
                std::ptr::null_mut(),
            );
            if h == INVALID_HANDLE_VALUE {
                return None;
            }
            let mut info: BY_HANDLE_FILE_INFORMATION = std::mem::zeroed();
            let ok = GetFileInformationByHandle(h, &mut info) != 0;
            CloseHandle(h);
            ok.then_some(FileId {
                volume: info.dwVolumeSerialNumber as u64,
                index: ((info.nFileIndexHigh as u64) << 32) | info.nFileIndexLow as u64,
                links: info.nNumberOfLinks as u64,
            })
        }
    }

    /// Soma dos fluxos alternativos de dados (ADS), sem o fluxo principal.
    pub fn alternate_streams(path: &Path) -> u64 {
        let w = long(path);
        let mut total = 0u64;
        unsafe {
            let mut data: WIN32_FIND_STREAM_DATA = std::mem::zeroed();
            let h = FindFirstStreamW(
                w.as_ptr(),
                FindStreamInfoStandard,
                &mut data as *mut _ as *mut _,
                0,
            );
            if h == INVALID_HANDLE_VALUE {
                return 0;
            }
            loop {
                let n = data.cStreamName.iter().position(|&c| c == 0).unwrap_or(0);
                let name = String::from_utf16_lossy(&data.cStreamName[..n]);
                if name != "::$DATA" {
                    total += data.StreamSize as u64;
                }
                if FindNextStreamW(h, &mut data as *mut _ as *mut _) == 0 {
                    break;
                }
            }
            FindClose(h);
        }
        total
    }

    pub fn owner(path: &Path) -> Option<String> {
        let w = long(path);
        unsafe {
            let mut sid: PSID = std::ptr::null_mut();
            let mut sd: PSECURITY_DESCRIPTOR = std::ptr::null_mut();
            let r = GetNamedSecurityInfoW(
                w.as_ptr(),
                SE_FILE_OBJECT,
                OWNER_SECURITY_INFORMATION,
                &mut sid,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                &mut sd,
            );
            if r != 0 {
                return None;
            }
            let name = sid_name(sid);
            LocalFree(sd as _);
            name
        }
    }

    unsafe fn sid_name(sid: PSID) -> Option<String> {
        let (mut nl, mut dl) = (256u32, 256u32);
        let mut name = vec![0u16; 256];
        let mut dom = vec![0u16; 256];
        let mut use_ = 0;
        if LookupAccountSidW(
            std::ptr::null(),
            sid,
            name.as_mut_ptr(),
            &mut nl,
            dom.as_mut_ptr(),
            &mut dl,
            &mut use_,
        ) == 0
        {
            return Some("(SID desconhecido)".into());
        }
        let n = String::from_utf16_lossy(&name[..nl as usize]);
        let d = String::from_utf16_lossy(&dom[..dl as usize]);
        Some(if d.is_empty() { n } else { format!("{d}\\{n}") })
    }

    /// Lista de controle de acesso (DACL). `None` se não puder ser lida.
    pub fn acl(path: &Path) -> Option<Vec<super::Ace>> {
        let w = long(path);
        unsafe {
            let mut dacl: *mut ACL = std::ptr::null_mut();
            let mut sd: PSECURITY_DESCRIPTOR = std::ptr::null_mut();
            let r = GetNamedSecurityInfoW(
                w.as_ptr(),
                SE_FILE_OBJECT,
                DACL_SECURITY_INFORMATION,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                &mut dacl,
                std::ptr::null_mut(),
                &mut sd,
            );
            if r != 0 {
                return None;
            }
            let mut out = Vec::new();
            if dacl.is_null() {
                // DACL nula = acesso total para todos.
                out.push(super::Ace {
                    account: "Todos".into(),
                    allow: true,
                    mask: 0x1000_0000,
                    inherited: false,
                });
            } else {
                let count = (*dacl).AceCount as u32;
                for i in 0..count {
                    let mut p: *mut core::ffi::c_void = std::ptr::null_mut();
                    if GetAce(dacl, i, &mut p) == 0 {
                        continue;
                    }
                    let hdr = &*(p as *const ACE_HEADER);
                    // 0 = ACCESS_ALLOWED_ACE, 1 = ACCESS_DENIED_ACE (mesmo layout: cabeçalho, máscara, SID)
                    if hdr.AceType > 1 {
                        continue;
                    }
                    let ace = &*(p as *const ACCESS_ALLOWED_ACE);
                    let sid = &ace.SidStart as *const u32 as PSID;
                    out.push(super::Ace {
                        account: sid_name(sid).unwrap_or_else(|| "?".into()),
                        allow: hdr.AceType == 0,
                        mask: ace.Mask,
                        inherited: hdr.AceFlags as u32 & INHERITED_ACE != 0,
                    });
                }
            }
            LocalFree(sd as _);
            Some(out)
        }
    }

    pub fn volumes() -> Vec<super::Location> {
        use super::{LocKind, Location};
        drives()
            .into_iter()
            .map(|d| {
                let w = wide(OsStr::new(&d));
                let kind = match unsafe { GetDriveTypeW(w.as_ptr()) } {
                    2 => LocKind::Removable,
                    4 => LocKind::Network,
                    5 => LocKind::Optical,
                    _ => LocKind::Disk,
                };
                let mut name = vec![0u16; 261];
                let mut fs = vec![0u16; 261];
                let ok = unsafe {
                    GetVolumeInformationW(
                        w.as_ptr(),
                        name.as_mut_ptr(),
                        261,
                        std::ptr::null_mut(),
                        std::ptr::null_mut(),
                        std::ptr::null_mut(),
                        fs.as_mut_ptr(),
                        261,
                    )
                } != 0;
                let cut = |b: &[u16]| {
                    String::from_utf16_lossy(&b[..b.iter().position(|&c| c == 0).unwrap_or(0)])
                };
                let label = if ok { cut(&name) } else { String::new() };
                // O Google Drive para computador monta uma unidade virtual chamada "Google Drive".
                let kind = if label.to_lowercase().contains("google drive") {
                    LocKind::Cloud
                } else {
                    kind
                };
                let label = if label.is_empty() {
                    kind.label().to_string()
                } else {
                    label
                };
                Location {
                    path: d,
                    label,
                    kind,
                }
            })
            .collect()
    }

    /// Unidades presentes (C:\, D:\ ...).
    pub fn drives() -> Vec<String> {
        let mask = unsafe { GetLogicalDrives() };
        (0..26)
            .filter(|i| mask & (1 << i) != 0)
            .map(|i| format!("{}:\\", (b'A' + i as u8) as char))
            .collect()
    }
}

#[cfg(unix)]
mod imp {
    use super::FileId;
    use crate::model::*;
    use std::fs::Metadata;
    use std::os::unix::fs::MetadataExt;
    use std::path::Path;

    pub fn attrs(md: &Metadata) -> u32 {
        let mut a = 0;
        if md.is_dir() {
            a |= ATTR_DIRECTORY;
        }
        if md.file_type().is_symlink() {
            a |= ATTR_REPARSE;
        }
        if md.permissions().readonly() {
            a |= ATTR_READONLY;
        }
        // Arquivo esparso: menos blocos do que o tamanho lógico.
        if md.is_file() && md.blocks() * 512 < md.size() {
            a |= ATTR_SPARSE;
        }
        a
    }

    pub fn times(md: &Metadata) -> (i64, i64, i64) {
        // Unix não tem data de criação portátil; usa ctime (mudança de status).
        (md.mtime(), md.atime(), md.ctime())
    }

    pub fn allocated(_path: &Path, md: &Metadata, _cluster: u64) -> u64 {
        md.blocks() * 512
    }

    pub fn cluster_size(path: &Path) -> u64 {
        std::fs::metadata(path)
            .map(|m| m.blksize())
            .unwrap_or(4096)
            .max(512)
    }

    pub fn volume_space(_path: &Path) -> (u64, u64) {
        // Sem libc/statvfs nesta versão.
        (0, 0)
    }

    pub fn file_id(path: &Path) -> Option<FileId> {
        let md = std::fs::symlink_metadata(path).ok()?;
        Some(FileId {
            volume: md.dev(),
            index: md.ino(),
            links: md.nlink(),
        })
    }

    pub fn alternate_streams(_path: &Path) -> u64 {
        0
    }

    pub fn owner(path: &Path) -> Option<String> {
        let uid = std::fs::symlink_metadata(path).ok()?.uid();
        let passwd = std::fs::read_to_string("/etc/passwd").unwrap_or_default();
        for line in passwd.lines() {
            let f: Vec<&str> = line.split(':').collect();
            if f.len() > 2 && f[2] == uid.to_string() {
                return Some(f[0].to_string());
            }
        }
        Some(format!("uid {uid}"))
    }

    /// Permissões Unix traduzidas para entradas no estilo ACL (dono, grupo, outros).
    pub fn acl(path: &Path) -> Option<Vec<super::Ace>> {
        let md = std::fs::symlink_metadata(path).ok()?;
        let mode = md.mode();
        let bits = |shift: u32| {
            let b = (mode >> shift) & 7;
            (if b & 4 != 0 { 0x1 } else { 0 })
                | (if b & 2 != 0 { 0x2 } else { 0 })
                | (if b & 1 != 0 { 0x20 } else { 0 })
        };
        let owner = owner(path).unwrap_or_else(|| format!("uid {}", md.uid()));
        Some(vec![
            super::Ace {
                account: owner,
                allow: true,
                mask: bits(6),
                inherited: false,
            },
            super::Ace {
                account: format!("grupo {}", md.gid()),
                allow: true,
                mask: bits(3),
                inherited: false,
            },
            super::Ace {
                account: "outros".into(),
                allow: true,
                mask: bits(0),
                inherited: false,
            },
        ])
    }

    pub fn volumes() -> Vec<super::Location> {
        drives()
            .into_iter()
            .map(|d| super::Location {
                label: d.clone(),
                path: d,
                kind: super::LocKind::Disk,
            })
            .collect()
    }

    pub fn drives() -> Vec<String> {
        let mut v = vec!["/".to_string()];
        if let Ok(home) = std::env::var("HOME") {
            v.push(home);
        }
        for m in ["/mnt", "/media", "/Volumes"] {
            if let Ok(rd) = std::fs::read_dir(m) {
                for e in rd.flatten() {
                    v.push(e.path().to_string_lossy().into_owned());
                }
            }
        }
        v
    }
}

pub use imp::{
    acl, allocated, alternate_streams, cluster_size, drives, file_id, owner, volume_space,
};

pub fn attrs(md: &Metadata) -> u32 {
    imp::attrs(md)
}

pub fn times(md: &Metadata) -> (i64, i64, i64) {
    imp::times(md)
}

/// Abre o gerenciador de arquivos com o item selecionado.
pub fn reveal(path: &Path) -> std::io::Result<()> {
    #[cfg(windows)]
    {
        let mut arg = std::ffi::OsString::from("/select,");
        arg.push(path.as_os_str());
        std::process::Command::new("explorer")
            .arg(arg)
            .spawn()
            .map(|_| ())
    }
    #[cfg(target_os = "macos")]
    {
        std::process::Command::new("open")
            .arg("-R")
            .arg(path)
            .spawn()
            .map(|_| ())
    }
    #[cfg(all(unix, not(target_os = "macos")))]
    {
        let dir = if path.is_dir() {
            path
        } else {
            path.parent().unwrap_or(path)
        };
        std::process::Command::new("xdg-open")
            .arg(dir)
            .spawn()
            .map(|_| ())
    }
}

/// Abre o arquivo com o programa padrão.
pub fn open(path: &Path) -> std::io::Result<()> {
    #[cfg(windows)]
    {
        // explorer.exe abre com o programa associado, sem passar o nome por um interpretador de comandos.
        std::process::Command::new("explorer")
            .arg(path)
            .spawn()
            .map(|_| ())
    }
    #[cfg(target_os = "macos")]
    {
        std::process::Command::new("open")
            .arg(path)
            .spawn()
            .map(|_| ())
    }
    #[cfg(all(unix, not(target_os = "macos")))]
    {
        std::process::Command::new("xdg-open")
            .arg(path)
            .spawn()
            .map(|_| ())
    }
}
