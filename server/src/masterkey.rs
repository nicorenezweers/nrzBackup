//! The master key: every backup is encrypted with it (age, X25519) before it leaves the device,
//! on top of Filen's own encryption. It is created once, shown only inside the app, and kept
//! sealed here so the in-app download can decrypt. Without nrzBackup a backup is restored with
//! the standard tool: `age -d -i hauptschluessel.txt datei.age > datei`.

use std::fs;
use std::io::{self, Read, Write};
use std::str::FromStr;

use age::secrecy::ExposeSecret;
use age::x25519::{Identity, Recipient};
use axum::body::Bytes;
use futures_util::{Stream, StreamExt, TryStreamExt};
use nrzbackup_shared::*;
use tokio::sync::mpsc;

use crate::AppState;
use crate::db::now;

/// File name suffix of encrypted backups.
pub const SUFFIX: &str = ".age";

/// Creates the master key on first start.
pub fn ensure(state: &AppState) {
    if state.db.get("master_key").is_some_and(|k| !k.is_empty()) {
        if identity(state).is_none() {
            state.db.event(
                EventLevel::Error,
                None,
                "Der Hauptschlüssel ist nicht lesbar (APP_SEED geändert?) – neue Backups warten",
            );
        }
        return;
    }
    let id = Identity::generate();
    let secret = id.to_string();
    state
        .db
        .set("master_key", &state.secrets.seal(secret.expose_secret()));
    state.db.set("master_key_public", &id.to_public().to_string());
    state.db.set("master_key_created", &now().to_string());
    state.db.set("master_key_confirmed", "0");
    state.db.event(
        EventLevel::Info,
        None,
        "Hauptschlüssel erstellt – bitte unter Einstellungen → Hauptschlüssel sichern",
    );
}

pub fn identity(state: &AppState) -> Option<Identity> {
    let sealed = state.db.get("master_key")?;
    let plain = state.secrets.open(&sealed)?;
    Identity::from_str(plain.trim()).ok()
}

/// The secret key as text (`AGE-SECRET-KEY-1…`), only for showing it in the app.
pub fn secret_text(state: &AppState) -> Option<String> {
    identity(state).map(|i| i.to_string().expose_secret().to_string())
}

pub fn status(state: &AppState) -> MasterKeyStatus {
    MasterKeyStatus {
        exists: identity(state).is_some(),
        public_key: state.db.get("master_key_public").unwrap_or_default(),
        confirmed: state.db.get("master_key_confirmed").as_deref() == Some("1"),
        created_at: state
            .db
            .get("master_key_created")
            .and_then(|s| s.parse().ok()),
    }
}

pub fn confirm(state: &AppState) {
    state.db.set("master_key_confirmed", "1");
    state.db.event(
        EventLevel::Info,
        None,
        "Hauptschlüssel als gesichert bestätigt",
    );
}

/// Encrypts `src` into `dst` (blocking; tests).
#[cfg(test)]
pub fn encrypt_file(recipient: &Recipient, src: &std::path::Path, dst: &std::path::Path) -> io::Result<()> {
    let enc = age::Encryptor::with_recipients(std::iter::once(recipient as &dyn age::Recipient))
        .map_err(io::Error::other)?;
    let mut input = io::BufReader::with_capacity(1 << 20, fs::File::open(src)?);
    let out = io::BufWriter::with_capacity(1 << 20, fs::File::create(dst)?);
    let mut w = enc.wrap_output(out)?;
    io::copy(&mut input, &mut w)?;
    let mut out = w.finish()?;
    out.flush()?;
    out.into_inner().map_err(|e| e.into_error())?.sync_all()
}

/// Writes everything sent on the channel age-encrypted into `path` (blocking thread). The
/// returned task ends with the result once the sender is dropped.
pub fn encrypting_writer(
    recipient: Recipient,
    path: std::path::PathBuf,
) -> io::Result<(tokio::task::JoinHandle<io::Result<()>>, mpsc::Sender<Bytes>)> {
    let file = fs::File::create(&path)?;
    let (tx, mut rx) = mpsc::channel::<Bytes>(16);
    let handle = tokio::task::spawn_blocking(move || {
        let enc = age::Encryptor::with_recipients(std::iter::once(&recipient as &dyn age::Recipient))
            .map_err(io::Error::other)?;
        let mut w = enc.wrap_output(io::BufWriter::with_capacity(1 << 20, file))?;
        while let Some(chunk) = rx.blocking_recv() {
            w.write_all(&chunk)?;
        }
        let mut out = w.finish()?;
        out.flush()?;
        out.into_inner().map_err(|e| e.into_error())?.sync_all()
    });
    Ok((handle, tx))
}

/// Decrypts a stream of an encrypted backup on the fly.
pub fn decrypt_stream<S, E>(
    identity: Identity,
    body: S,
) -> impl Stream<Item = Result<Bytes, io::Error>> + Send + 'static
where
    S: Stream<Item = Result<Bytes, E>> + Send + Unpin + 'static,
    E: Into<io::Error> + Send + 'static,
{
    let (tx, rx) = mpsc::channel::<Result<Bytes, io::Error>>(4);
    let reader = tokio_util::io::StreamReader::new(body.map_err(Into::into));
    tokio::task::spawn_blocking(move || {
        let run = || -> io::Result<()> {
            let sync = tokio_util::io::SyncIoBridge::new(reader);
            let dec = age::Decryptor::new(sync)
                .map_err(|e| io::Error::other(format!("Kein gültiges age-Backup: {e}")))?;
            let mut r = dec
                .decrypt(std::iter::once(&identity as &dyn age::Identity))
                .map_err(|e| io::Error::other(format!("Entschlüsseln fehlgeschlagen: {e}")))?;
            let mut buf = vec![0u8; 1 << 20];
            loop {
                let n = r.read(&mut buf)?;
                if n == 0 {
                    return Ok(());
                }
                if tx.blocking_send(Ok(Bytes::copy_from_slice(&buf[..n]))).is_err() {
                    return Ok(()); // client gone
                }
            }
        };
        if let Err(e) = run() {
            let _ = tx.blocking_send(Err(e));
        }
    });
    futures_util::stream::unfold(rx, |mut rx| async move {
        rx.recv().await.map(|item| (item, rx))
    })
    .boxed()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn roundtrip() {
        let dir = tempfile::tempdir().unwrap();
        let (src, dst) = (dir.path().join("a"), dir.path().join("a.age"));
        let data: Vec<u8> = (0..3_000_000u32).map(|i| (i % 251) as u8).collect();
        fs::write(&src, &data).unwrap();
        let id = Identity::generate();
        encrypt_file(&id.to_public(), &src, &dst).unwrap();
        let enc = fs::read(&dst).unwrap();
        assert!(enc.starts_with(b"age-encryption.org/v1"));
        assert_ne!(&enc[..], &data[..]);
        // stream in odd chunks like a download
        let chunks: Vec<Result<Bytes, io::Error>> =
            enc.chunks(777_777).map(|c| Ok(Bytes::copy_from_slice(c))).collect();
        let mut out = vec![];
        let mut s = decrypt_stream(id, futures_util::stream::iter(chunks));
        while let Some(c) = s.next().await {
            out.extend_from_slice(&c.unwrap());
        }
        assert_eq!(out, data);
        // streaming writer gives the same plaintext back
        let id2 = Identity::generate();
        let p2 = dir.path().join("b.age");
        let (h, tx) = encrypting_writer(id2.to_public(), p2.clone()).unwrap();
        for c in data.chunks(100_003) {
            tx.send(Bytes::copy_from_slice(c)).await.unwrap();
        }
        drop(tx);
        h.await.unwrap().unwrap();
        let enc2 = fs::read(&p2).unwrap();
        let mut out2 = vec![];
        let mut s2 = decrypt_stream(id2, futures_util::stream::iter(vec![Ok::<_, io::Error>(Bytes::from(enc2))]));
        while let Some(c) = s2.next().await {
            out2.extend_from_slice(&c.unwrap());
        }
        assert_eq!(out2, data);
        // wrong key fails
        let chunks: Vec<Result<Bytes, io::Error>> = vec![Ok(Bytes::from(enc))];
        let mut s = decrypt_stream(Identity::generate(), futures_util::stream::iter(chunks));
        assert!(s.next().await.unwrap().is_err());
    }
}
