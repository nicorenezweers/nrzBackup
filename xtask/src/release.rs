//! Releases: Umbrel-Dateien auf dem Zweig `umbrel` (Liefer-Weg des nrzStore) und das
//! Multi-Arch-Image fuer die nrzGit-Registry.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use crate::{Result, bundle, output, run};

const REGISTRY: &str = "100.78.107.162:8390";
const IMAGE: &str = "100.78.107.162:8390/nicorenezweers/nrz-backup";
/// (docker-Architektur, Rust-Ziel); glibc 2.36 = debian bookworm und neuer.
const TARGETS: [(&str, &str); 2] =
    [("amd64", "x86_64-unknown-linux-gnu"), ("arm64", "aarch64-unknown-linux-gnu")];

/// Image fuer umbrelOS (linux/amd64 + linux/arm64). Server und Web-App werden vorher auf dem
/// Host gebaut (cargo-zigbuild + dx), das Image packt sie nur – kein Compiler, keine Emulation.
const DOCKERFILE: &str = r#"FROM debian:trixie-slim@sha256:f6b08b09a05725f9431502d3b5714f5c4576e82c1a84fa1debe0c24a7e0c3ec4

ARG TARGETARCH
COPY --chmod=755 dist/nrzbackup-server-${TARGETARCH} /usr/local/bin/nrzbackup-server
COPY dist/public /app/public

ENV NRZBACKUP_ADDR=0.0.0.0:8080 \
    NRZBACKUP_DATA=/data \
    NRZBACKUP_PUBLIC=/app/public

# umbrelOS creates app directories as 1000:1000
USER 1000:1000
EXPOSE 8080
HEALTHCHECK --interval=30s --timeout=5s --start-period=10s CMD ["nrzbackup-server", "healthcheck"]
CMD ["nrzbackup-server"]
"#;

/// Temporaeres Verzeichnis, das beim Verlassen geloescht wird.
struct TempDir(PathBuf);

impl TempDir {
    fn new(tag: &str) -> Result<Self> {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or_default();
        let dir = std::env::temp_dir().join(format!("nrzbackup-{tag}-{}-{nanos}", std::process::id()));
        std::fs::create_dir_all(&dir).map_err(|e| format!("{}: {e}", dir.display()))?;
        Ok(Self(dir))
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn check_version(root: &Path, version: &str) -> Result {
    let toml = std::fs::read_to_string(root.join("server/Cargo.toml")).map_err(|e| e.to_string())?;
    if toml.lines().any(|l| l == format!("version = \"{version}\"")) {
        Ok(())
    } else {
        Err(format!("server/Cargo.toml hat nicht Version {version}"))
    }
}

fn zigbuild(root: &Path, target: &str) -> Result<PathBuf> {
    run(Command::new("cargo").current_dir(root).args([
        "zigbuild", "--release", "-p", "nrzbackup-server", "--target", &format!("{target}.2.36"),
    ]))?;
    Ok(root.join("target").join(target).join("release/nrzbackup-server"))
}

/// Baut nrzBackup fuer umbrelOS und legt den Release-Stand auf dem Zweig `umbrel` ab:
/// `nrzbackup-server-amd64.gz`, `nrzbackup-server-arm64.gz`, `public.tar.gz`, `SHA256SUMS`.
/// Danach: `git push origin umbrel umbrel-<version>` (nrzGit aktualisiert den nrzStore).
pub fn umbrel(root: &Path, version: &str, notes: Option<&str>) -> Result {
    let tag = format!("umbrel-{version}");
    let notes = notes.map(str::to_owned).unwrap_or_else(|| format!("nrzBackup {version}"));
    let git = || {
        let mut c = Command::new("git");
        c.current_dir(root);
        c
    };

    if !output(git().args(["status", "--porcelain", "--untracked-files=no"]))?.trim().is_empty() {
        return Err("Arbeitsverzeichnis nicht sauber".into());
    }
    check_version(root, version)?;

    let out = TempDir::new("umbrel")?;
    bundle(root)?;
    run(Command::new("tar")
        .arg("-czf")
        .arg(out.0.join("public.tar.gz"))
        .arg("-C")
        .arg(root.join("target/dx/nrzbackup/release/web"))
        .arg("public"))?;
    for (arch, target) in TARGETS {
        let bin = zigbuild(root, target)?;
        let gz = std::fs::File::create(out.0.join(format!("nrzbackup-server-{arch}.gz")))
            .map_err(|e| e.to_string())?;
        run(Command::new("gzip").arg("-9").arg("-c").arg(&bin).stdout(gz))?;
    }
    let sums = output(Command::new("shasum").current_dir(&out.0).args([
        "-a", "256", "nrzbackup-server-amd64.gz", "nrzbackup-server-arm64.gz", "public.tar.gz",
    ]))?;
    std::fs::write(out.0.join("SHA256SUMS"), sums).map_err(|e| e.to_string())?;

    let src = output(git().args(["rev-parse", "--short", "HEAD"]))?.trim().to_owned();
    let wt = TempDir::new("worktree")?;
    std::fs::remove_dir(&wt.0).map_err(|e| e.to_string())?;
    if git().args(["show-ref", "--quiet", "refs/heads/umbrel"]).status().is_ok_and(|s| s.success()) {
        run(git().args(["worktree", "add", "-q"]).arg(&wt.0).arg("umbrel"))?;
    } else {
        run(git().args(["worktree", "add", "-q", "--detach"]).arg(&wt.0))?;
        run(Command::new("git").current_dir(&wt.0).args(["checkout", "-q", "--orphan", "umbrel"]))?;
        run(Command::new("git").current_dir(&wt.0).args(["rm", "-rqf", "."]))?;
    }
    let result = (|| {
        for entry in std::fs::read_dir(&out.0).map_err(|e| e.to_string())? {
            let entry = entry.map_err(|e| e.to_string())?;
            std::fs::copy(entry.path(), wt.0.join(entry.file_name())).map_err(|e| e.to_string())?;
        }
        std::fs::write(
            wt.0.join("README.md"),
            format!(
                "# nrzBackup – Umbrel-Dateien\n\n\
                 Nur fertige Dateien fuer die App `nrz-backup` im nrzStore (geladen von `hooks/pre-start`).\n\
                 Quellcode: Zweig `main`. Stand: {src}, Version {version}.\n"
            ),
        )
        .map_err(|e| e.to_string())?;
        let in_wt = || {
            let mut c = Command::new("git");
            c.current_dir(&wt.0);
            c
        };
        run(in_wt().args(["add", "-A"]))?;
        run(in_wt().args(["commit", "-q", "-m", &format!("nrzBackup {version}: Umbrel-Dateien (aus {src})")]))?;
        run(in_wt().args(["tag", "-a", &tag, "-m", &notes]))
    })();
    run(git().args(["worktree", "remove", "--force"]).arg(&wt.0))?;
    result?;
    println!("Fertig: Zweig umbrel und Tag {tag}. Jetzt: git push origin umbrel {tag}");
    Ok(())
}

/// Registry-Anmeldung: `$NRZGIT_REGISTRY_AUTH` (base64 von user:token) oder der
/// git-extraheader fuer die Registry aus `GIT_CONFIG_KEY_n`/`GIT_CONFIG_VALUE_n`.
fn registry_auth() -> Option<String> {
    if let Some(a) = std::env::var("NRZGIT_REGISTRY_AUTH").ok().filter(|a| !a.is_empty()) {
        return Some(a);
    }
    let count: usize = std::env::var("GIT_CONFIG_COUNT").ok()?.parse().ok()?;
    (0..count).rev().find_map(|i| {
        let key = std::env::var(format!("GIT_CONFIG_KEY_{i}")).ok()?;
        let lower = key.to_ascii_lowercase();
        if !(key.contains(REGISTRY) && lower.ends_with("extraheader")) {
            return None;
        }
        let value = std::env::var(format!("GIT_CONFIG_VALUE_{i}")).ok()?;
        Some(value.rsplit("Basic ").next()?.trim().to_owned())
    })
}

/// Multi-Arch-Image `IMAGE:<version>` in die nrzGit-Registry; nrzGit aktualisiert danach
/// Image (Tag + Digest), Version und Release-Notiz im nrzStore.
pub fn image(root: &Path, version: &str) -> Result {
    check_version(root, version)?;

    // ── bauen ──
    let dist = root.join("dist");
    let _ = std::fs::remove_dir_all(root.join("target/dx/nrzbackup/release/web/public"));
    let _ = std::fs::remove_dir_all(&dist);
    bundle(root)?;
    std::fs::create_dir_all(&dist).map_err(|e| e.to_string())?;
    run(Command::new("cp")
        .arg("-R")
        .arg(root.join("target/dx/nrzbackup/release/web/public"))
        .arg(dist.join("public")))?;
    for (arch, target) in TARGETS {
        let bin = zigbuild(root, target)?;
        std::fs::copy(&bin, dist.join(format!("nrzbackup-server-{arch}"))).map_err(|e| e.to_string())?;
    }

    // ── Registry-Anmeldung (temporaere docker-Konfiguration, ~/.docker bleibt unberuehrt) ──
    let auth = registry_auth().ok_or("Keine Registry-Anmeldung (NRZGIT_REGISTRY_AUTH setzen)")?;
    let orig = std::env::var_os("DOCKER_CONFIG")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| Path::new(&h).join(".docker")))
        .ok_or("HOME nicht gesetzt")?;
    let cfg_dir = TempDir::new("docker")?;
    // Kontext (colima), CLI-Plugins und buildx-Builder der normalen Konfiguration behalten
    for d in ["contexts", "cli-plugins", "buildx"] {
        if orig.join(d).exists() {
            std::os::unix::fs::symlink(orig.join(d), cfg_dir.0.join(d)).map_err(|e| e.to_string())?;
        }
    }
    let mut cfg: serde_json::Value = std::fs::read_to_string(orig.join("config.json"))
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .filter(serde_json::Value::is_object)
        .unwrap_or_else(|| serde_json::json!({}));
    let obj = cfg.as_object_mut().expect("object");
    obj.remove("credsStore");
    let auths = obj.entry("auths").or_insert_with(|| serde_json::json!({}));
    if !auths.is_object() {
        *auths = serde_json::json!({});
    }
    auths[REGISTRY] = serde_json::json!({ "auth": auth });
    std::fs::write(cfg_dir.0.join("config.json"), cfg.to_string()).map_err(|e| e.to_string())?;
    let docker = || {
        let mut c = Command::new("docker");
        c.current_dir(root).env("DOCKER_CONFIG", &cfg_dir.0);
        c
    };

    // ── Image ──
    let builder_exists = docker()
        .args(["buildx", "inspect", "nrzbackup-builder"])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_ok_and(|s| s.success());
    if !builder_exists {
        run(docker()
            .args(["buildx", "create", "--name", "nrzbackup-builder", "--driver", "docker-container"])
            .args(["--config", "docker/buildkitd.toml"])
            .stdout(Stdio::null()))?;
    }
    let image = format!("{IMAGE}:{version}");
    let mut child = docker()
        .args(["buildx", "build", "--builder", "nrzbackup-builder", "--platform", "linux/amd64,linux/arm64"])
        .args(["-f", "-", "-t", &image, "--push", "."])
        .stdin(Stdio::piped())
        .spawn()
        .map_err(|e| format!("docker: {e}"))?;
    child
        .stdin
        .take()
        .expect("stdin")
        .write_all(DOCKERFILE.as_bytes())
        .map_err(|e| e.to_string())?;
    let status = child.wait().map_err(|e| e.to_string())?;
    if !status.success() {
        return Err(format!("docker buildx build endete mit {status}"));
    }
    run(docker().args(["buildx", "imagetools", "inspect", "--builder", "nrzbackup-builder", &image]))?;
    println!("Fertig: {image}");
    Ok(())
}
