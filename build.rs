// Сборка C++-шва (libgeodesk + gol_ffi.cpp) при feature-флаге `gol-ffi`.
//
// Требования (при включённом флаге):
//   - компилятор C++20 (clang++ или g++),
//   - исходники libgeodesk и заголовки gtl, доступные по:
//     * env LIBGEODESK_DIR / GTL_DIR, либо
//     * каталоги vendor/libgeodesk и vendor/gtl.

use std::path::{Path, PathBuf};

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
    let g = vendor.join("libgeodesk");
    let t = vendor.join("gtl");
    if g.join("include").exists() && g.join("src").exists() && t.join("include").exists() {
        return (g, t);
    }

    panic!(
        "feature `gol-ffi` требует исходники libgeodesk и gtl.\n\
         Задайте переменные LIBGEODESK_DIR и GTL_DIR, либо склонируйте их:\n\
         \x20 git clone https://github.com/clarisma/libgeodesk.git vendor/libgeodesk\n\
         \x20 git clone https://github.com/greg7mdp/gtl.git vendor/gtl"
    );
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
