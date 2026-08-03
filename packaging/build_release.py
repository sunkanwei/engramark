#!/usr/bin/env python3
"""Build Engramark release archives around the native Rust binary.

One archive per target: engramark-<version>-<target>.tar.gz (POSIX) or .zip
(Windows), plus a shared checksums.txt. The built binary is probed on the
build host before packaging; cross-built artifacts must additionally pass the
native capability probe on their own runners before release.
"""
from __future__ import annotations

import argparse
import gzip
import hashlib
import json
import os
import re
import stat
import shutil
import subprocess
import tarfile
import tempfile
import time
import zipfile
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
RUST = ROOT / "rust"
TARGETS = {
    "macos-arm64": ("aarch64-apple-darwin", "tar.gz", "engramark"),
    "macos-x86_64": ("x86_64-apple-darwin", "tar.gz", "engramark"),
    "linux-x86_64": ("x86_64-unknown-linux-gnu", "tar.gz", "engramark"),
    "windows-x86_64": ("x86_64-pc-windows-msvc", "zip", "engramark.exe"),
}
PUBLIC_FILES = (
    "README.md",
    "README.zh-CN.md",
    "THIRD_PARTY_NOTICES.md",
    "THIRD_PARTY_NOTICES.zh-CN.md",
    "VERSION",
    "engramark.json",
    "install.sh",
    "install.ps1",
)
PUBLIC_DIRECTORIES = ("adapters", "assets", "docs", "examples")
PACKAGE_SCRIPTS = {
    "bin/install.sh": 0o755,
    "bin/uninstall": 0o755,
    "bin/uninstall.sh": 0o755,
    "bin/uninstall.ps1": 0o644,
    "install.sh": 0o755,
}


def sha256(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as handle:
        for chunk in iter(lambda: handle.read(1024 * 1024), b""):
            digest.update(chunk)
    return digest.hexdigest()


def cargo_build(rust_target: str) -> Path:
    env = dict(os.environ)
    # SQLite compile options enter the cache fingerprint; keep the environment
    # from silently changing them between builds.
    for key in list(env):
        if key.startswith(("SQLITE_", "LIBSQLITE3", "RUSTFLAGS", "CARGO_TARGET_")):
            env.pop(key)
    if rust_target.endswith("apple-darwin"):
        env["MACOSX_DEPLOYMENT_TARGET"] = "13.0"
    result = subprocess.run(
        ["cargo", "build", "--release", "--locked", "--target", rust_target],
        cwd=RUST, env=env,
    )
    if result.returncode:
        raise SystemExit(f"cargo build failed for {rust_target}")
    return RUST / "target" / rust_target / "release"


def probe(binary: Path, work: Path) -> None:
    env = dict(os.environ, ENGRAMARK_HOME=str(work))
    for args in (["migrate-v1"], ["rebuild"], ["diagnose", "--full"]):
        result = subprocess.run([str(binary), *args], capture_output=True, text=True,
                                env=env, timeout=120)
        if result.returncode:
            raise SystemExit(f"capability probe failed: {binary} {args}: {result.stderr.strip()}")


def copy_public(destination: Path) -> None:
    for relative in PUBLIC_FILES:
        shutil.copy2(ROOT / relative, destination / relative)
    license_path = ROOT / "LICENSE"
    if license_path.is_file():
        shutil.copy2(license_path, destination / "LICENSE")
    for relative in PUBLIC_DIRECTORIES:
        shutil.copytree(
            ROOT / relative,
            destination / relative,
            ignore=shutil.ignore_patterns("__pycache__", "*.pyc", ".DS_Store"),
        )
    (destination / "bin").mkdir(exist_ok=True)
    for relative, mode in PACKAGE_SCRIPTS.items():
        source = ROOT / relative
        if source.is_file():
            target = destination / relative
            target.parent.mkdir(parents=True, exist_ok=True)
            shutil.copy2(source, target)
            target.chmod(mode)


def cargo_metadata(rust_target: str) -> dict:
    tree = subprocess.run(
        ["cargo", "metadata", "--format-version", "1", "--locked", "--filter-platform", rust_target],
        cwd=RUST, capture_output=True, text=True,
    )
    if tree.returncode:
        raise SystemExit(f"cargo metadata failed for {rust_target}: {tree.stderr.strip()}")
    return json.loads(tree.stdout)


def git_value(*args: str) -> str:
    result = subprocess.run(["git", *args], cwd=ROOT, capture_output=True, text=True)
    return result.stdout.strip() if result.returncode == 0 else ""


def write_sbom(destination: Path, version: str, target: str, rust_target: str,
               metadata: dict, epoch: int) -> None:
    packages = sorted(
        (
            {"name": pkg["name"], "version": pkg["version"], "license": pkg.get("license") or ""}
            for pkg in metadata["packages"]
        ),
        key=lambda item: (item["name"], item["version"]),
    )
    sbom = {
        "sbom_format": 1,
        "package": "engramark",
        "version": version,
        "target": target,
        "rust_target": rust_target,
        "build": {
            "source_revision": git_value("rev-parse", "HEAD"),
            "source_dirty": bool(git_value("status", "--porcelain")),
            "source_date_epoch": epoch,
            "profile": "release",
            "lto": "thin",
            "codegen_units": 1,
            "panic": "abort",
            "opt_level": 3,
            "target_cpu": "Rust target default (target-cpu=native forbidden)",
            "macos_deployment_target": "13.0" if rust_target.endswith("apple-darwin") else None,
            "linux_max_glibc": "2.35" if rust_target == "x86_64-unknown-linux-gnu" else None,
        },
        "dependencies": packages,
    }
    (destination / "SBOM.json").write_text(
        json.dumps(sbom, ensure_ascii=False, indent=2) + "\n", encoding="utf-8")


def copy_dependency_licenses(destination: Path, metadata: dict) -> None:
    # Keep this distinct from the root LICENSE on case-insensitive filesystems.
    licenses = destination / "third-party-licenses"
    crates = licenses / "crates"
    crates.mkdir(parents=True)
    shutil.copy2(RUST / "assets" / "unicode" / "unicode-license.txt",
                 licenses / "unicode-license.txt")
    for package in sorted(metadata["packages"], key=lambda item: (item["name"], item["version"])):
        if package["name"] == "engramark":
            continue
        root = Path(package["manifest_path"]).parent
        files = sorted(
            path for path in root.iterdir()
            if path.is_file() and path.name.upper().startswith(("LICENSE", "COPYING", "NOTICE"))
        )
        if not package.get("license") or not files:
            raise SystemExit(
                f"dependency license evidence missing: {package['name']} {package['version']}")
        target = crates / f"{package['name']}-{package['version']}"
        target.mkdir()
        for source in files:
            shutil.copy2(source, target / source.name)


def write_file_manifest(destination: Path) -> None:
    lines = []
    casefolded_paths: dict[str, str] = {}
    for path in sorted(destination.rglob("*"), key=lambda item: item.relative_to(destination).as_posix()):
        relative = path.relative_to(destination).as_posix()
        previous = casefolded_paths.setdefault(relative.casefold(), relative)
        if previous != relative:
            raise SystemExit(
                f"release staging has a case-insensitive path collision: {previous} / {relative}")
        if path.is_symlink():
            raise SystemExit(f"release staging contains symlink: {relative}")
        if path.is_dir():
            lines.append(f"d\t0\t-\t{relative}\n")
        elif path.is_file():
            lines.append(f"f\t{path.stat().st_size}\t{sha256(path)}\t{relative}\n")
        else:
            raise SystemExit(f"release staging contains special file: {relative}")
    (destination / "MANIFEST.tsv").write_text("".join(lines), encoding="utf-8")


def source_date_epoch() -> int:
    configured = os.environ.get("SOURCE_DATE_EPOCH")
    if configured:
        try:
            value = int(configured)
        except ValueError as exc:
            raise SystemExit("SOURCE_DATE_EPOCH must be an integer") from exc
        if value < 0:
            raise SystemExit("SOURCE_DATE_EPOCH must be non-negative")
        return value
    result = subprocess.run(
        ["git", "log", "-1", "--format=%ct"], cwd=ROOT, capture_output=True, text=True)
    return int(result.stdout.strip()) if result.returncode == 0 else 0


def tar_filter(epoch: int):
    def normalize(info: tarfile.TarInfo) -> tarfile.TarInfo:
        info.uid = 0
        info.gid = 0
        info.uname = ""
        info.gname = ""
        info.mtime = epoch
        if info.isdir():
            info.mode = 0o755
        elif info.isfile():
            info.mode = 0o755 if info.name.endswith(
                ("/engramark", "/install.sh", "/uninstall", "/uninstall.sh")) else 0o644
        return info
    return normalize


def write_archive(stage: Path, archive: Path, archive_kind: str, epoch: int) -> None:
    if archive_kind == "zip":
        zip_epoch = max(epoch, 315532800)  # ZIP timestamps start at 1980-01-01.
        timestamp = time.gmtime(zip_epoch)[:6]
        with zipfile.ZipFile(archive, "w", zipfile.ZIP_DEFLATED, compresslevel=9) as bundle:
            for path in [stage, *sorted(stage.rglob("*"))]:
                relative = path.relative_to(stage.parent).as_posix()
                if path.is_dir():
                    relative += "/"
                info = zipfile.ZipInfo(relative, timestamp)
                mode = 0o755 if path.is_dir() or path.name in {
                    "engramark", "install.sh", "uninstall", "uninstall.sh"} else 0o644
                info.external_attr = ((stat.S_IFDIR if path.is_dir() else stat.S_IFREG) | mode) << 16
                info.compress_type = zipfile.ZIP_DEFLATED
                bundle.writestr(info, b"" if path.is_dir() else path.read_bytes())
        return
    with archive.open("wb") as raw:
        with gzip.GzipFile(filename="", mode="wb", fileobj=raw, mtime=epoch, compresslevel=9) as compressed:
            with tarfile.open(fileobj=compressed, mode="w", format=tarfile.PAX_FORMAT) as bundle:
                bundle.add(stage, arcname="engramark", recursive=True, filter=tar_filter(epoch))


def verify_binary_platform_contract(binary: Path, rust_target: str) -> None:
    if rust_target.endswith("apple-darwin"):
        result = subprocess.run(
            ["xcrun", "vtool", "-show-build", str(binary)], capture_output=True, text=True)
        if result.returncode or "minos 13.0" not in result.stdout:
            raise SystemExit(
                "macOS binary does not declare the fixed 13.0 deployment target:\n"
                + result.stdout + result.stderr)
    elif rust_target == "x86_64-unknown-linux-gnu":
        result = subprocess.run(
            ["readelf", "--version-info", str(binary)], capture_output=True, text=True)
        if result.returncode:
            raise SystemExit("readelf failed while checking the Linux ABI: " + result.stderr)
        versions = {
            tuple(int(part) for part in match.split("."))
            for match in re.findall(r"GLIBC_([0-9]+(?:\.[0-9]+)+)", result.stdout)
        }
        if versions and max(versions) > (2, 35):
            raise SystemExit(f"Linux binary requires GLIBC_{'.'.join(map(str, max(versions)))} (> 2.35)")


def build(target: str, output: Path) -> Path:
    rust_target, archive_kind, binary_name = TARGETS[target]
    version = (ROOT / "VERSION").read_text(encoding="utf-8").strip()
    release = cargo_build(rust_target)
    binary = release / binary_name
    if not binary.is_file():
        raise SystemExit(f"missing built binary: {binary}")
    verify_binary_platform_contract(binary, rust_target)
    output.mkdir(parents=True, exist_ok=True)
    archive = output / f"engramark-{version}-{target}.{archive_kind}"

    host_info = subprocess.run(["rustc", "-vV"], capture_output=True, text=True, check=True).stdout
    host_target = next(
        (line.removeprefix("host: ") for line in host_info.splitlines() if line.startswith("host: ")),
        "",
    )
    can_run = rust_target == host_target
    epoch = source_date_epoch()
    with tempfile.TemporaryDirectory(prefix="engramark-release-") as temporary:
        stage = Path(temporary) / "engramark"
        stage.mkdir()
        copy_public(stage)
        (stage / "bin").mkdir(exist_ok=True)
        shutil.copy2(binary, stage / "bin" / binary_name)
        (stage / "bin" / binary_name).chmod(0o755)
        metadata = cargo_metadata(rust_target)
        write_sbom(stage, version, target, rust_target, metadata, epoch)
        copy_dependency_licenses(stage, metadata)
        write_file_manifest(stage)
        if can_run and os.environ.get("ENGRAMARK_SKIP_PROBE") != "1":
            probe(binary, Path(temporary) / "probe-home")
        write_archive(stage, archive, archive_kind, epoch)
    checksums = output / "checksums.txt"
    lines = {}
    if checksums.exists():
        for line in checksums.read_text().splitlines():
            digest, name = line.split(None, 1)
            lines[name] = digest
    lines[archive.name] = sha256(archive)
    checksums.write_text(
        "".join(f"{digest} {name}\n" for name, digest in sorted(lines.items())),
        encoding="utf-8")
    print(f"Built {archive} ({archive.stat().st_size} bytes)")
    print(f"SHA256 {lines[archive.name]}")
    return archive


def main() -> int:
    parser = argparse.ArgumentParser(description="Build Engramark release archives")
    parser.add_argument("--target", choices=[*TARGETS], default="macos-arm64")
    parser.add_argument("--output", type=Path, default=ROOT / "dist")
    args = parser.parse_args()
    build(args.target, args.output)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
