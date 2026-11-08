#!/usr/bin/env python3
"""Monta o APK do AtlasDisco sem Gradle: cargo ndk → aapt2 → bibliotecas → zipalign → apksigner.

Requer: Android SDK (build-tools, platforms) e NDK, `cargo-ndk`, Java (keytool/apksigner).
Variáveis: ANDROID_HOME, ANDROID_NDK_HOME; para assinatura de release, ATLASDISCO_KEYSTORE (arquivo),
ATLASDISCO_KEYSTORE_PASS e ATLASDISCO_KEY_ALIAS. Sem elas, usa uma chave de depuração local.

Uso: python scripts/build_apk.py [--abis arm64-v8a,armeabi-v7a,x86_64] [--out AtlasDisco-android.apk]
"""
import argparse
import os
import re
import shutil
import struct
import subprocess
import sys
import zipfile
import zlib
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
TRIPLES = {"arm64-v8a": "aarch64-linux-android", "armeabi-v7a": "armv7-linux-androideabi", "x86_64": "x86_64-linux-android"}
WIN = os.name == "nt"


def run(*cmd, **kw):
    print("+", " ".join(str(c) for c in cmd), flush=True)
    subprocess.run([str(c) for c in cmd], check=True, **kw)


def newest(d: Path) -> Path:
    vs = sorted((p for p in d.iterdir() if p.is_dir()), key=lambda p: [int(x) if x.isdigit() else 0 for x in re.split(r"[.-]", p.name)])
    if not vs:
        sys.exit(f"nada em {d}")
    return vs[-1]


def tool(bt: Path, name: str) -> Path:
    for ext in ([".exe", ".bat"] if WIN else [""]):
        p = bt / (name + ext)
        if p.exists():
            return p
    sys.exit(f"{name} não encontrado em {bt}")


def png(size: int) -> bytes:
    """Ícone (blocos de um mapa de árvore) desenhado sem bibliotecas externas."""
    bg = (0x14, 0x1A, 0x24)
    blocks = [  # x0, y0, x1, y1 em 0..256, cor
        (36, 36, 140, 156, (0x3F, 0x6F, 0xD6)), (140, 36, 220, 100, (0xD9, 0x4F, 0x4F)), (140, 100, 220, 156, (0x3B, 0x9E, 0x5F)),
        (36, 156, 100, 220, (0xE0, 0x8E, 0x2B)), (100, 156, 172, 220, (0x8E, 0x5C, 0xC9)), (172, 156, 220, 220, (0x2A, 0xA6, 0xB5)),
    ]
    rows = []
    for y in range(size):
        row = bytearray([0])
        for x in range(size):
            u, v = (x + 0.5) * 256 / size, (y + 0.5) * 256 / size
            # cantos arredondados do fundo
            r, cx, cy = 48, min(max(u, 56), 200), min(max(v, 56), 200)
            inside = (u - cx) ** 2 + (v - cy) ** 2 <= r * r  # retângulo 8..248 com raio 48
            c, a = (0, 0, 0), 0
            if inside:
                c, a = bg, 255
                for x0, y0, x1, y1, col in blocks:
                    if x0 + 3 <= u <= x1 - 3 and y0 + 3 <= v <= y1 - 3:
                        c = col
            row += bytes((*c, a))
        rows.append(bytes(row))
    raw = zlib.compress(b"".join(rows), 9)

    def chunk(t, d):
        return struct.pack(">I", len(d)) + t + d + struct.pack(">I", zlib.crc32(t + d) & 0xFFFFFFFF)

    return b"\x89PNG\r\n\x1a\n" + chunk(b"IHDR", struct.pack(">IIBBBBB", size, size, 8, 6, 0, 0, 0)) + chunk(b"IDAT", raw) + chunk(b"IEND", b"")


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--abis", default="arm64-v8a,armeabi-v7a,x86_64")
    ap.add_argument("--out", default=str(ROOT / "target" / "AtlasDisco-android.apk"))
    ap.add_argument("--skip-build", action="store_true")
    a = ap.parse_args()
    abis = [x.strip() for x in a.abis.split(",") if x.strip()]

    sdk = Path(os.environ.get("ANDROID_HOME") or os.environ.get("ANDROID_SDK_ROOT") or sys.exit("defina ANDROID_HOME"))
    bt = newest(sdk / "build-tools")
    platform = newest(sdk / "platforms")
    android_jar = platform / "android.jar"

    cargo = (ROOT / "Cargo.toml").read_text(encoding="utf-8")
    version = re.search(r'^version\s*=\s*"([^"]+)"', cargo, re.M).group(1)
    major, minor, patch = (int(x) for x in re.findall(r"\d+", version)[:3])
    code = major * 10000 + minor * 100 + patch

    work = ROOT / "target" / "apk"
    shutil.rmtree(work, ignore_errors=True)
    (work / "res").mkdir(parents=True)

    if not a.skip_build:
        args = ["cargo", "ndk", "-P", "26"]
        for abi in abis:
            args += ["-t", abi]
        run(*args, "build", "--release", "-p", "atlasdisco-android", cwd=ROOT)

    # recursos: ícones por densidade
    for name, px in {"mdpi": 48, "hdpi": 72, "xhdpi": 96, "xxhdpi": 144, "xxxhdpi": 192}.items():
        d = work / "res" / f"mipmap-{name}"
        d.mkdir()
        (d / "ic_launcher.png").write_bytes(png(px))
    manifest = (ROOT / "android" / "AndroidManifest.xml").read_text(encoding="utf-8")
    manifest = manifest.replace("@VERSION_CODE@", str(code)).replace("@VERSION_NAME@", version)
    (work / "AndroidManifest.xml").write_text(manifest, encoding="utf-8")

    aapt2 = tool(bt, "aapt2")
    run(aapt2, "compile", "--dir", work / "res", "-o", work / "res.zip")
    unaligned = work / "unaligned.apk"
    run(aapt2, "link", "-o", unaligned, "-I", android_jar, "--manifest", work / "AndroidManifest.xml", work / "res.zip")

    with zipfile.ZipFile(unaligned, "a", zipfile.ZIP_DEFLATED) as z:
        for abi in abis:
            so = ROOT / "target" / TRIPLES[abi] / "release" / "libatlasdisco.so"
            if not so.exists():
                sys.exit(f"faltou {so}")
            z.write(so, f"lib/{abi}/libatlasdisco.so")

    aligned = work / "aligned.apk"
    run(tool(bt, "zipalign"), "-f", "-P", "16", "4", unaligned, aligned)

    ks = os.environ.get("ATLASDISCO_KEYSTORE")
    if ks:
        pw, alias = os.environ["ATLASDISCO_KEYSTORE_PASS"], os.environ["ATLASDISCO_KEY_ALIAS"]
    else:
        ks = str(ROOT / "target" / "debug.keystore")
        pw, alias = "android", "androiddebugkey"
        if not Path(ks).exists():
            run("keytool", "-genkeypair", "-keystore", ks, "-storepass", pw, "-keypass", pw, "-alias", alias, "-keyalg", "RSA", "-keysize", "2048",
                "-validity", "10000", "-dname", "CN=AtlasDisco Debug")
        print("AVISO: assinado com chave de depuração (defina ATLASDISCO_KEYSTORE para release)")
    out = Path(a.out)
    out.parent.mkdir(parents=True, exist_ok=True)
    run(tool(bt, "apksigner"), "sign", "--ks", ks, "--ks-pass", f"pass:{pw}", "--ks-key-alias", alias, "--out", out, aligned)
    run(tool(bt, "apksigner"), "verify", "--print-certs", out)
    print(f"APK: {out} ({out.stat().st_size / 1048576:.1f} MB, versão {version}/{code})")


if __name__ == "__main__":
    main()
