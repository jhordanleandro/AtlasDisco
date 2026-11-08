//! Formatação no padrão brasileiro (vírgula decimal, ponto de milhar).

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Unit {
    #[default]
    Auto,
    B,
    KB,
    MB,
    GB,
    TB,
}

pub fn size(b: u64) -> String {
    size_in(b, Unit::Auto, 1)
}

pub fn size_in(b: u64, unit: Unit, dec: usize) -> String {
    const N: [&str; 5] = ["B", "KB", "MB", "GB", "TB"];
    let i = match unit {
        Unit::Auto => {
            let (mut v, mut i) = (b as f64, 0);
            while v >= 1024.0 && i < 4 {
                v /= 1024.0;
                i += 1;
            }
            i
        }
        Unit::B => 0,
        Unit::KB => 1,
        Unit::MB => 2,
        Unit::GB => 3,
        Unit::TB => 4,
    };
    if i == 0 {
        return format!("{} B", int(b as i64));
    }
    let v = b as f64 / 1024f64.powi(i as i32);
    format!("{} {}", dec_br(v, dec), N[i])
}

pub fn signed(d: i64) -> String {
    let s = size(d.unsigned_abs());
    match d.signum() {
        1 => format!("+{s}"),
        -1 => format!("−{s}"),
        _ => s,
    }
}

pub fn int(n: i64) -> String {
    let s = n.unsigned_abs().to_string();
    let mut out = String::new();
    for (i, c) in s.chars().enumerate() {
        if i > 0 && (s.len() - i).is_multiple_of(3) {
            out.push('.');
        }
        out.push(c);
    }
    if n < 0 {
        format!("-{out}")
    } else {
        out
    }
}

pub fn dec_br(v: f64, dec: usize) -> String {
    let s = format!("{:.*}", dec, v.abs());
    let (ip, fp) = s.split_once('.').unwrap_or((&s, ""));
    let ip = int(ip.parse::<i64>().unwrap_or(0));
    let sign = if v < 0.0 && v.abs() >= 0.5 * 10f64.powi(-(dec as i32)) {
        "-"
    } else {
        ""
    };
    if fp.is_empty() {
        format!("{sign}{ip}")
    } else {
        format!("{sign}{ip},{fp}")
    }
}

pub fn pct(p: f64) -> String {
    format!("{} %", dec_br(p * 100.0, 1))
}

pub fn date(ts: i64) -> String {
    if ts <= 0 {
        return String::new();
    }
    use chrono::{Local, TimeZone};
    match Local.timestamp_opt(ts, 0).single() {
        Some(d) => d.format("%d/%m/%Y %H:%M").to_string(),
        None => String::new(),
    }
}

pub fn attrs(a: u32) -> String {
    let mut s = String::new();
    for (bit, c) in [
        (0x1, 'R'),
        (0x2, 'H'),
        (0x4, 'S'),
        (0x10, 'D'),
        (0x20, 'A'),
        (0x800, 'C'),
        (0x200, 'P'),
        (0x100, 'T'),
        (0x1000, 'O'),
        (0x400, 'L'),
        (0x4000, 'E'),
    ] {
        if a & bit != 0 {
            s.push(c);
        }
    }
    s
}

/// "10 MB", "1,5GB", "500" → bytes.
pub fn parse_size(s: &str) -> Option<u64> {
    let s = s.trim().to_uppercase().replace(' ', "");
    if s.is_empty() {
        return None;
    }
    let units = [
        ("TB", 1u64 << 40),
        ("GB", 1 << 30),
        ("MB", 1 << 20),
        ("KB", 1 << 10),
        ("B", 1),
    ];
    let (num, mul) = units
        .iter()
        .find_map(|(u, m)| s.strip_suffix(u).map(|n| (n.to_string(), *m)))
        .unwrap_or((s.clone(), 1));
    num.replace(',', ".")
        .parse::<f64>()
        .ok()
        .map(|v| (v * mul as f64) as u64)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn formatos() {
        assert_eq!(size(512), "512 B");
        assert_eq!(size(1536), "1,5 KB");
        assert_eq!(size(5 * 1024 * 1024 * 1024), "5,0 GB");
        assert_eq!(int(1234567), "1.234.567");
        assert_eq!(parse_size("1,5 GB"), Some(1610612736));
        assert_eq!(parse_size("10mb"), Some(10485760));
        assert_eq!(pct(0.4567), "45,7 %");
    }
}
