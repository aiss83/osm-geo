// Сборка C++-шва (libgeodesk + gol_ffi.cpp) при feature-флаге `gol-ffi`.
//
// Требования (при включённом флаге):
//   - компилятор C++20 (clang++ или g++),
//   - исходники libgeodesk и заголовки gtl. Поиск по приоритету:
//       1. переменные окружения LIBGEODESK_DIR / GTL_DIR,
//       2. каталоги vendor/libgeodesk и vendor/gtl,
//       3. автоматическое клонирование в vendor/ (нужен git и сеть).

use std::path::{Path, PathBuf};
use std::process::Command;

fn main() {
    println!("cargo:rerun-if-changed=gol-ffi/gol_ffi.cpp");
    println!("cargo:rerun-if-changed=gol-ffi/gol_ffi.h");

    if std::env::var("CARGO_FEATURE_GOL_FFI").is_err() {
        return;
    }

    let (libgeodesk, gtl) = locate_deps();
    build(&libgeodesk, &gtl);
}

fn locate_deps() -> (PathBuf, PathBuf) {
    if let (Ok(g), Ok(t)) = (
        std::env::var("LIBGEODESK_DIR"),
        std::env::var("GTL_DIR"),
    ) {
        return (PathBuf::from(g), PathBuf::from(t));
    }

    let vendor = Path::new("vendor");
    let libgeodesk = vendor.join("libgeodesk");
    let gtl = vendor.join("gtl");

    ensure_cloned(
        "https://github.com/clarisma/libgeodesk.git",
        None,
        &libgeodesk,
        |d| d.join("include").exists() && d.join("src").exists(),
    );
    ensure_cloned(
        "https://github.com/greg7mdp/gtl.git",
        Some("v1.2.0"),
        &gtl,
        |d| d.join("include").exists(),
    );

    (libgeodesk, gtl)
}

/// Склонировать репозиторий в `dest`, если он ещё не готов к использованию.
fn ensure_cloned<F: Fn(&Path) -> bool>(
    url: &str,
    branch: Option<&str>,
    dest: &Path,
    is_ready: F,
) {
    if is_ready(dest) {
        return;
    }

    println!(
        "cargo:warning=gol-ffi: клонирование {} в {} ...",
        url,
        dest.display()
    );

    let mut cmd = Command::new("git");
    cmd.args(["clone", "--depth", "1"]);
    if let Some(b) = branch {
        cmd.args(["--branch", b]);
    }
    cmd.arg(url).arg(dest);
    let ok = cmd.status().map(|s| s.success()).unwrap_or(false);

    if !ok || !is_ready(dest) {
        panic!(
            "feature `gol-ffi` требует исходники libgeodesk и gtl, \
             но их не удалось клонировать автоматически (нужны git и сеть).\n\
             Клонируйте вручную:\n\
             \x20 git clone https://github.com/clarisma/libgeodesk.git vendor/libgeodesk\n\
             \x20 git clone --branch v1.2.0 https://github.com/greg7mdp/gtl.git vendor/gtl"
        );
    }
}

fn build(libgeodesk: &Path, gtl: &Path) {
    let mut sources = Vec::new();
    collect_cpp(&libgeodesk.join("src"), &mut sources);
    sources.push(PathBuf::from("gol-ffi/gol_ffi.cpp"));

    let mut build = cc::Build::new();
    build
        .cpp(true)
        .flag("-std=c++20")
        .flag("-O2")
        .flag_if_supported("-Wno-deprecated-declarations")
        .include(libgeodesk.join("include"))
        .include(libgeodesk.join("src"))
        .include(gtl.join("include"))
        .warnings(false);

    for source in &sources {
        build.file(source);
    }

    build.compile("golffi");
}

fn collect_cpp(dir: &Path, out: &mut Vec<PathBuf>) {
    if let Ok(entries) = std::fs::read_dir(dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                collect_cpp(&path, out);
            } else if path.extension().and_then(|s| s.to_str()) == Some("cpp") {
                out.push(path);
            }
        }
    }
}
