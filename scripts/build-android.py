"""Build a signed Android APK with the Android SDK, NDK and the workspace Rust toolchain."""
import argparse
import hashlib
import os
from pathlib import Path
import platform
import shutil
import subprocess
import tempfile
import tomllib
import urllib.request
import zipfile

ROOT = Path(__file__).resolve().parents[1]
ANDROID = ROOT / "android"
API, MIN_API = 36, 26
MPV_RELEASE = "https://github.com/media-kit/libmpv-android-video-build/releases/download/v1.1.7"
TARGETS = {
    "arm64-v8a": ("aarch64-linux-android", "4363dfa5d3d415b91c1f16f6fb90c3fe59a77dfd3f9b824d2b24b492d6b09df9"),
    "x86_64": ("x86_64-linux-android", "90268cd15f0766e07fb8e427388c621161177c9eb343c544f327bd63232bb236"),
}
EXE = ".exe" if os.name == "nt" else ""


def run(*args, env=None):
    subprocess.run([str(a) for a in args], cwd=ROOT, env=env, check=True)


def sha256(path):
    with path.open("rb") as f:
        return hashlib.file_digest(f, "sha256").hexdigest()


def native_archive(cache, abi):
    name = f"default-{abi}.jar"
    path = cache / name
    digest = TARGETS[abi][1]
    if path.is_file() and sha256(path) == digest:
        return path
    with tempfile.NamedTemporaryFile(dir=cache, delete=False) as temp:
        download = Path(temp.name)
    try:
        with urllib.request.urlopen(f"{MPV_RELEASE}/{name}", timeout=120) as response, download.open("wb") as out:
            shutil.copyfileobj(response, out)
        if sha256(download) != digest:
            raise ValueError(f"SHA-256 mismatch for {name}")
        download.replace(path)
    finally:
        download.unlink(missing_ok=True)
    return path


def build_native(abi, llvm, cache, temp, apk):
    rust, _ = TARGETS[abi]
    cc_key = rust.replace("-", "_")
    cargo_key = "CARGO_TARGET_" + cc_key.upper()
    target = f"--target={rust}{MIN_API}"
    target_dir = Path(os.getenv("CARGO_TARGET_DIR", ROOT / "build/android/cargo")).resolve()
    env = os.environ.copy()
    bin_dir = llvm / "bin"
    resource = next((llvm / "lib/clang").glob("*/include")).parent
    # bindgen parses a shell-style argument string even on Windows.
    env.update({
        "ANDROID_NDK": os.environ["ANDROID_NDK_HOME"],
        "PATH": str(bin_dir) + os.pathsep + env["PATH"],
        "CARGO_TARGET_DIR": str(target_dir),
        cargo_key + "_LINKER": str(bin_dir / ("clang" + EXE)),
        cargo_key + "_RUSTFLAGS": f"-C link-arg={target} -C link-arg=-Wl,-z,max-page-size=16384,-z,defs -C link-arg=-lc++_static -C link-arg=-lc++abi",
        "CC_" + cc_key: str(bin_dir / ("clang" + EXE)),
        "CXX_" + cc_key: str(bin_dir / ("clang++" + EXE)),
        "AR_" + cc_key: str(bin_dir / ("llvm-ar" + EXE)),
        "CFLAGS_" + cc_key: target,
        "CXXFLAGS_" + cc_key: target,
        "BINDGEN_EXTRA_CLANG_ARGS": f'{target} --sysroot="{(llvm / "sysroot").as_posix()}" -resource-dir "{resource.as_posix()}"',
    })
    run("cargo", "build", "--locked", "--release", "--lib", "--target", rust, "-p", "music-app", env=env)
    library = "libmusic_app_ui.so"
    stripped = temp / f"{abi}-{library}"
    run(bin_dir / ("llvm-strip" + EXE), "--strip-unneeded", "-o", stripped, target_dir / rust / "release" / library)
    apk.write(stripped, f"lib/{abi}/{library}")
    archive = native_archive(cache, abi)
    entry = f"lib/{abi}/libmpv.so"
    with zipfile.ZipFile(archive) as jar, jar.open(entry) as src, apk.open(entry, "w") as dest:
        shutil.copyfileobj(src, dest)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--abi", choices=TARGETS, action="append", help="repeat for a universal APK; default arm64-v8a")
    args = parser.parse_args()
    abis = list(dict.fromkeys(args.abi or ["arm64-v8a"]))
    sdk = Path(os.environ.get("ANDROID_HOME") or os.environ["ANDROID_SDK_ROOT"])
    ndk = Path(os.environ["ANDROID_NDK_HOME"])
    revision = (ndk / "source.properties").read_text("utf-8")
    if "Pkg.Revision = 30.0.16248370" not in revision:
        parser.error("set ANDROID_NDK_HOME to NDK 30.0.16248370 (the Skia binary requires its libc++)")
    host = {"Windows": "windows", "Linux": "linux", "Darwin": "darwin"}[platform.system()]
    llvm = ndk / f"toolchains/llvm/prebuilt/{host}-x86_64"
    jbin = Path(os.environ["JAVA_HOME"]) / "bin"
    java = jbin / ("java" + EXE)
    tools = sdk / "build-tools/36.0.0"
    android_jar = sdk / f"platforms/android-{API}/android.jar"
    aapt = tools / ("aapt2" + EXE)
    align = tools / ("zipalign" + EXE)
    signer = tools / "lib/apksigner.jar"
    keystore = Path(os.environ["MUSIC_ANDROID_KEYSTORE"]).resolve(strict=True)
    if not os.environ.get("MUSIC_ANDROID_STORE_PASSWORD"):
        parser.error("set MUSIC_ANDROID_STORE_PASSWORD for the release keystore")
    version = tomllib.loads((ROOT / "Cargo.toml").read_text("utf-8"))["workspace"]["package"]["version"]
    major, minor, patch = map(int, version.split("."))
    if not (0 <= minor < 1000 and 0 <= patch < 1000):
        raise ValueError("Android versionCode requires minor and patch below 1000")
    code = str(major * 1_000_000 + minor * 1000 + patch)
    flavor = abis[0] if len(abis) == 1 else "universal"
    output = ROOT / "dist/android" / f"MusicPlayer-android-{flavor}-{version}.apk"
    output.parent.mkdir(parents=True, exist_ok=True)
    cache = Path(os.getenv("MUSIC_ANDROID_BUILD_DIR", ROOT / "build/android")).resolve()
    cache.mkdir(parents=True, exist_ok=True)
    with tempfile.TemporaryDirectory(dir=output.parent) as temporary:
        t = Path(temporary)
        compiled, base, aligned, signed = [t / s for s in ("res.zip", "base.apk", "aligned.apk", "signed.apk")]
        classes, dex = t / "classes", t / "dex"
        classes.mkdir()
        dex.mkdir()
        run(aapt, "compile", "--dir", ANDROID / "res", "-o", compiled)
        run(aapt, "link", "-I", android_jar, "--manifest", ANDROID / "AndroidManifest.xml",
            "--min-sdk-version", str(MIN_API), "--target-sdk-version", str(API), "--version-code", code,
            "--version-name", version, "-o", base, compiled)
        run(jbin / ("javac" + EXE), "-encoding", "UTF-8", "--release", "8", "-classpath", android_jar,
            "-d", classes, *sorted((ANDROID / "src").rglob("*.java")))
        with zipfile.ZipFile(t / "classes.jar", "w") as jar:
            for item in classes.rglob("*.class"):
                jar.write(item, item.relative_to(classes).as_posix())
        run(java, "-cp", tools / "lib/d8.jar", "com.android.tools.r8.D8", "--release", "--lib", android_jar,
            "--min-api", str(MIN_API), "--output", dex, t / "classes.jar")
        with zipfile.ZipFile(base, "a", compression=zipfile.ZIP_DEFLATED) as apk:
            apk.write(dex / "classes.dex", "classes.dex")
            apk.write(ROOT / "THIRD_PARTY_NOTICES.md", "assets/licenses/THIRD_PARTY_NOTICES.md")
            for item in sorted((ANDROID / "licenses").glob("*")):
                apk.write(item, "assets/licenses/" + item.name)
            for abi in abis:
                build_native(abi, llvm, cache, t, apk)
        run(align, "-f", "-P", "16", "4", base, aligned)
        key_pass = "MUSIC_ANDROID_KEY_PASSWORD" if os.environ.get("MUSIC_ANDROID_KEY_PASSWORD") else "MUSIC_ANDROID_STORE_PASSWORD"
        run(java, "-jar", signer, "sign", "--ks", keystore, "--ks-key-alias", os.getenv("MUSIC_ANDROID_KEY_ALIAS", "music-player"),
            "--ks-pass", "env:MUSIC_ANDROID_STORE_PASSWORD", "--key-pass", "env:" + key_pass, "--out", signed, aligned)
        run(java, "-jar", signer, "verify", "--verbose", "--print-certs", signed)
        run(align, "-c", "-P", "16", "4", signed)
        signed.replace(output)
    print(f"{sha256(output)}  {output}")


if __name__ == "__main__":
    main()
