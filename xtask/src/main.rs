//! Bau- und Release-Aufgaben fuer nrzBackup:
//!
//! ```text
//! cargo xtask web                          nrzUI-Thema + index.html fuer dx schreiben
//! cargo xtask bundle                       Web-App bauen (web + dx bundle)
//! cargo xtask umbrel-release 1.0.9 "Neu"   Release-Stand auf Zweig `umbrel` + Tag
//! cargo xtask image 1.0.8                  Multi-Arch-Image in die nrzGit-Registry
//! ```
//!
//! Voraussetzungen: cargo-zigbuild + zig, Dioxus-CLI 0.7 (`$DX` oder `~/.cargo/bin/dx`),
//! fuer `image` zusaetzlich docker mit buildx.

mod release;
mod web;

use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode};

type Result<T = ()> = std::result::Result<T, String>;

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let root = root();
    let res = match args.first().map(String::as_str) {
        Some("web") => write_web(&root),
        Some("bundle") => bundle(&root),
        Some("umbrel-release") => match args.get(1) {
            Some(v) => release::umbrel(&root, v, args.get(2).map(String::as_str)),
            None => Err("Version angeben, z. B. 1.0.9".into()),
        },
        Some("image") => match args.get(1) {
            Some(v) => release::image(&root, v),
            None => Err("Version angeben, z. B. 1.0.8".into()),
        },
        _ => Err("Aufruf: cargo xtask <web | bundle | umbrel-release VERSION [NOTIZ] | image VERSION>".into()),
    };
    match res {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("xtask: {e}");
            ExitCode::FAILURE
        }
    }
}

/// Stamm des Workspaces (eine Ebene ueber `xtask/`).
fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).parent().expect("workspace root").to_path_buf()
}

/// Schreibt die Dateien, die `dx` im Crate-Stamm erwartet (nur wenn sich etwas geaendert hat,
/// damit `dx serve` nicht unnoetig neu baut).
pub fn write_web(root: &Path) -> Result {
    for (name, content) in [("tailwind.css", web::TAILWIND_CSS), ("index.html", web::INDEX_HTML)] {
        let path = root.join(name);
        if std::fs::read_to_string(&path).ok().as_deref() != Some(content) {
            std::fs::write(&path, content).map_err(|e| format!("{name}: {e}"))?;
        }
    }
    Ok(())
}

/// Baut die Web-App nach `target/dx/nrzbackup/release/web/public`.
pub fn bundle(root: &Path) -> Result {
    write_web(root)?;
    run(Command::new(dx()).current_dir(root).args([
        "bundle", "--platform", "web", "--release", "--package", "nrzbackup",
    ]))
}

fn dx() -> PathBuf {
    if let Some(dx) = std::env::var_os("DX") {
        return dx.into();
    }
    let home = std::env::var_os("CARGO_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| Path::new(&h).join(".cargo")))
        .unwrap_or_default();
    let dx = home.join("bin/dx");
    if dx.exists() { dx } else { "dx".into() }
}

/// Fuehrt einen Befehl aus und meldet einen Fehler, wenn er nicht erfolgreich endet.
pub fn run(cmd: &mut Command) -> Result {
    let status = cmd.status().map_err(|e| format!("{:?}: {e}", cmd.get_program()))?;
    if status.success() { Ok(()) } else { Err(format!("{cmd:?} endete mit {status}")) }
}

/// Wie [`run`], liefert aber die Standardausgabe.
pub fn output(cmd: &mut Command) -> Result<String> {
    let out = cmd.output().map_err(|e| format!("{:?}: {e}", cmd.get_program()))?;
    if !out.status.success() {
        return Err(format!("{cmd:?} endete mit {}: {}", out.status, String::from_utf8_lossy(&out.stderr).trim()));
    }
    Ok(String::from_utf8_lossy(&out.stdout).into_owned())
}
