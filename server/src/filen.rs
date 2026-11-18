//! Filen.io client: login (auth versions 2 and 3), client-side encryption and the few
//! drive operations nrzBackup needs (folders, chunked upload, download, delete, quota).
//!
//! Follows https://docs.filen.io/docs/api/ and the official TypeScript SDK
//! (`@filen/sdk`): files are split into 1 MiB chunks, every chunk is encrypted with
//! AES-256-GCM using a random per-file key; names and metadata are encrypted with the
//! user's master key ("002" metadata). Nothing readable ever leaves this machine.
//! Uploaded files are normal Filen files and show up in every Filen app.

use std::collections::HashMap;
use std::path::Path;
use std::sync::Mutex;
use std::time::Duration;

use aes_gcm::aead::{Aead, KeyInit};
use aes_gcm::{Aes256Gcm, Nonce};
use base64::Engine;
use base64::engine::general_purpose::STANDARD as B64;
use futures_util::{TryStreamExt, stream};
use hmac::{Hmac, Mac};
use rand::Rng;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha1::Sha1;
use sha2::{Digest, Sha256, Sha512};
use tokio::io::AsyncReadExt;

pub const CHUNK_SIZE: usize = 1024 * 1024;
const UPLOAD_THREADS: usize = 4;
const CHUNK_RETRIES: u32 = 4;

const GATEWAYS: [&str; 8] = [
    "https://gateway.filen.io",
    "https://gateway.filen.net",
    "https://gateway.filen-1.net",
    "https://gateway.filen-2.net",
    "https://gateway.filen-3.net",
    "https://gateway.filen-4.net",
    "https://gateway.filen-5.net",
    "https://gateway.filen-6.net",
];
const INGESTS: [&str; 8] = [
    "https://ingest.filen.io",
    "https://ingest.filen.net",
    "https://ingest.filen-1.net",
    "https://ingest.filen-2.net",
    "https://ingest.filen-3.net",
    "https://ingest.filen-4.net",
    "https://ingest.filen-5.net",
    "https://ingest.filen-6.net",
];
const EGESTS: [&str; 8] = [
    "https://egest.filen.io",
    "https://egest.filen.net",
    "https://egest.filen-1.net",
    "https://egest.filen-2.net",
    "https://egest.filen-3.net",
    "https://egest.filen-4.net",
    "https://egest.filen-5.net",
    "https://egest.filen-6.net",
];

/// Characters of Filen's random strings (keys, nonces, upload keys).
const CHARSET: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";

/// Everything needed to work with the account after login. Stored sealed in the DB.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Session {
    pub email: String,
    pub auth_version: u32,
    pub api_key: String,
    /// All master keys, the current one last (auth v3: just the DEK).
    pub master_keys: Vec<String>,
    /// Decrypted private key (base64 DER); auth v3 derives the name-hash key from it.
    pub private_key: String,
    pub base_folder: String,
}

#[derive(Debug, Clone)]
pub struct Uploaded {
    pub uuid: String,
    pub key: String,
    pub region: String,
    pub bucket: String,
    pub chunks: u64,
    pub size: u64,
}

/// A folder as listed by [`Filen::list_dir`].
#[derive(Debug, Clone)]
pub struct DirFolder {
    pub uuid: String,
    pub name: String,
    pub modified: Option<i64>,
}

/// A file as listed by [`Filen::list_dir`], with everything needed to download it.
#[derive(Debug, Clone)]
pub struct DirFile {
    pub uuid: String,
    pub name: String,
    pub size: u64,
    pub mime: String,
    pub key: String,
    pub modified: Option<i64>,
    pub region: String,
    pub bucket: String,
    pub chunks: u64,
    pub version: u64,
}

pub struct Filen {
    http: reqwest::Client,
    /// (parent uuid, lowercase name) → folder uuid
    dirs: Mutex<HashMap<(String, String), String>>,
}

type R<T> = Result<T, String>;

impl Filen {
    pub fn new() -> Self {
        let http = reqwest::Client::builder()
            .user_agent(concat!("nrzBackup/", env!("CARGO_PKG_VERSION")))
            .connect_timeout(Duration::from_secs(20))
            .timeout(Duration::from_secs(300))
            .build()
            .expect("http client");
        Filen {
            http,
            dirs: Mutex::new(HashMap::new()),
        }
    }

    pub fn forget_dirs(&self) {
        self.dirs.lock().unwrap().clear();
    }

    // ─────────────────────────── HTTP ───────────────────────────

    async fn api(
        &self,
        method: &str,
        path: &str,
        api_key: Option<&str>,
        body: Option<&Value>,
    ) -> R<Value> {
        let mut last = String::new();
        for attempt in 0..3u32 {
            if attempt > 0 {
                tokio::time::sleep(Duration::from_millis(800 * u64::from(attempt))).await;
            }
            let url = format!("{}{}", pick(&GATEWAYS), path);
            let mut req = if method == "GET" {
                self.http.get(&url)
            } else {
                self.http.post(&url)
            };
            req = req
                .header("Accept", "application/json")
                .bearer_auth(api_key.unwrap_or("anonymous"));
            if let Some(b) = body {
                let text = serde_json::to_string(b).map_err(|e| e.to_string())?;
                req = req
                    .header("Content-Type", "application/json")
                    .header("Checksum", sha512_hex(text.as_bytes()))
                    .body(text);
            }
            let res = match req.send().await {
                Ok(r) => r,
                Err(e) => {
                    last = format!("Filen nicht erreichbar: {e}");
                    continue;
                }
            };
            let status = res.status();
            let text = res.text().await.unwrap_or_default();
            if status.is_server_error() || status.as_u16() == 429 {
                last = format!("Filen HTTP {status}");
                continue;
            }
            let v: Value = serde_json::from_str(&text).map_err(|_| {
                format!(
                    "Filen HTTP {status}: {}",
                    text.chars().take(200).collect::<String>()
                )
            })?;
            if v.get("status").and_then(Value::as_bool) != Some(true) {
                let msg = v
                    .get("message")
                    .and_then(Value::as_str)
                    .unwrap_or("Unbekannter Fehler");
                let code = v.get("code").and_then(Value::as_str).unwrap_or("");
                return Err(if code.is_empty() {
                    format!("Filen: {msg}")
                } else {
                    format!("Filen: {msg} ({code})")
                });
            }
            return Ok(v.get("data").cloned().unwrap_or(Value::Null));
        }
        Err(last)
    }

    async fn post(&self, s: &Session, path: &str, body: Value) -> R<Value> {
        self.api("POST", path, Some(&s.api_key), Some(&body)).await
    }

    async fn get(&self, s: &Session, path: &str) -> R<Value> {
        self.api("GET", path, Some(&s.api_key), None).await
    }

    // ─────────────────────────── Login ───────────────────────────

    pub async fn login(&self, email: &str, password: &str, two_factor: &str) -> R<Session> {
        let email = email.trim();
        let info = self
            .api(
                "POST",
                "/v3/auth/info",
                None,
                Some(&json!({ "email": email })),
            )
            .await?;
        let auth_version = info.get("authVersion").and_then(Value::as_u64).unwrap_or(2) as u32;
        let salt = info
            .get("salt")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string();

        let (master_key, derived_password) = tokio::task::spawn_blocking({
            let password = password.to_string();
            move || derive_password_and_master_key(&password, &salt, auth_version)
        })
        .await
        .map_err(|e| e.to_string())??;

        let code = two_factor.trim();
        let login = self
            .api(
                "POST",
                "/v3/login",
                None,
                Some(&json!({
                    "email": email,
                    "password": derived_password,
                    "twoFactorCode": if code.is_empty() { "XXXXXX" } else { code },
                    "authVersion": auth_version,
                })),
            )
            .await?;
        let api_key = str_field(&login, "apiKey")?;

        let mut s = Session {
            email: email.to_string(),
            auth_version,
            api_key,
            master_keys: vec![],
            private_key: String::new(),
            base_folder: String::new(),
        };
        s.base_folder = str_field(&self.get(&s, "/v3/user/baseFolder").await?, "uuid")?;

        s.master_keys = if auth_version == 3 {
            // v3: the derived key only unlocks the data encryption key (DEK).
            let dek = self.get(&s, "/v3/user/dek").await?;
            let Some(enc) = dek
                .get("dek")
                .and_then(Value::as_str)
                .filter(|d| !d.is_empty())
            else {
                return Err("Für dieses Filen-Konto gibt es noch keinen Schlüssel. Bitte zuerst einmal in einer offiziellen Filen-App anmelden.".into());
            };
            vec![metadata_decrypt(enc, &[master_key])?]
        } else {
            let enc = metadata_encrypt(&master_key, &master_key);
            let res = self
                .post(&s, "/v3/user/masterKeys", json!({ "masterKeys": enc }))
                .await?;
            let mut keys = vec![master_key.clone()];
            if let Some(stored) = res.get("keys").and_then(Value::as_str)
                && let Ok(list) = metadata_decrypt(stored, &keys)
            {
                for k in list.split('|') {
                    if !k.is_empty() && !keys.iter().any(|x| x == k) {
                        keys.push(k.to_string());
                    }
                }
            }
            keys
        };

        // Private key: needed for the salted name hashes of auth v3.
        let pair = self
            .get(&s, "/v3/user/keyPair/info")
            .await
            .unwrap_or(Value::Null);
        if let Some(enc) = pair
            .get("privateKey")
            .and_then(Value::as_str)
            .filter(|k| k.len() > 16)
        {
            s.private_key = metadata_decrypt(enc, &s.master_keys).unwrap_or_default();
        }
        if auth_version == 3 && s.private_key.is_empty() {
            return Err("Der private Schlüssel des Filen-Kontos fehlt. Bitte zuerst einmal in einer offiziellen Filen-App anmelden.".into());
        }
        Ok(s)
    }

    /// `(used, max)` bytes.
    pub async fn quota(&self, s: &Session) -> R<(u64, u64)> {
        let v = self.get(s, "/v3/user/info").await?;
        let n = |k: &str| v.get(k).and_then(Value::as_f64).unwrap_or(0.0).max(0.0) as u64;
        Ok((n("storageUsed"), n("maxStorage")))
    }

    // ─────────────────────────── Folders ───────────────────────────

    /// Folder uuid for `segments` below the root, created when missing.
    pub async fn ensure_path(&self, s: &Session, segments: &[&str]) -> R<String> {
        let mut parent = s.base_folder.clone();
        for seg in segments.iter().map(|x| x.trim()).filter(|x| !x.is_empty()) {
            parent = self.ensure_dir(s, &parent, seg).await?;
        }
        Ok(parent)
    }

    async fn ensure_dir(&self, s: &Session, parent: &str, name: &str) -> R<String> {
        let cache_key = (parent.to_string(), name.to_lowercase());
        if let Some(u) = self.dirs.lock().unwrap().get(&cache_key) {
            return Ok(u.clone());
        }
        let name_hashed = name_hash(s, name)?;
        let exists = self
            .post(
                s,
                "/v3/dir/exists",
                json!({ "parent": parent, "nameHashed": name_hashed }),
            )
            .await?;
        let uuid = if exists.get("exists").and_then(Value::as_bool) == Some(true) {
            str_field(&exists, "uuid")?
        } else {
            let uuid = uuid::Uuid::new_v4().to_string();
            let enc = metadata_encrypt(&json!({ "name": name }).to_string(), s.current_key());
            self.post(
                s,
                "/v3/dir/create",
                json!({ "uuid": uuid, "name": enc, "nameHashed": name_hashed, "parent": parent }),
            )
            .await?;
            uuid
        };
        self.dirs.lock().unwrap().insert(cache_key, uuid.clone());
        Ok(uuid)
    }

    // ─────────────────────────── Explorer ───────────────────────────

    /// Folders and files directly inside folder `uuid`, names decrypted.
    pub async fn list_dir(&self, s: &Session, uuid: &str) -> R<(Vec<DirFolder>, Vec<DirFile>)> {
        let v = self.post(s, "/v3/dir/content", json!({ "uuid": uuid })).await?;
        let arr = |k: &str| v.get(k).and_then(Value::as_array).cloned().unwrap_or_default();
        let num = |x: &Value, k: &str| x.get(k).and_then(Value::as_f64).unwrap_or(0.0).max(0.0);
        let mut folders = Vec::new();
        for f in arr("folders") {
            let Some(id) = f.get("uuid").and_then(Value::as_str) else { continue };
            let name = f
                .get("name")
                .and_then(Value::as_str)
                .and_then(|n| metadata_decrypt(n, &s.master_keys).ok())
                .and_then(|j| serde_json::from_str::<Value>(&j).ok())
                .and_then(|j| j.get("name").and_then(Value::as_str).map(str::to_string))
                .unwrap_or_else(|| "(unlesbarer Name)".into());
            let ts = num(&f, "timestamp");
            folders.push(DirFolder {
                uuid: id.to_string(),
                name,
                modified: (ts > 0.0).then(|| (ts * 1000.0) as i64),
            });
        }
        let mut files = Vec::new();
        for f in arr("uploads") {
            let Some(id) = f.get("uuid").and_then(Value::as_str) else { continue };
            let meta = f
                .get("metadata")
                .and_then(Value::as_str)
                .and_then(|m| metadata_decrypt(m, &s.master_keys).ok())
                .and_then(|j| serde_json::from_str::<Value>(&j).ok())
                .unwrap_or(Value::Null);
            let mstr = |k: &str| meta.get(k).and_then(Value::as_str).unwrap_or("").to_string();
            let last = meta
                .get("lastModified")
                .and_then(Value::as_f64)
                .filter(|n| *n > 0.0)
                .unwrap_or(num(&f, "timestamp") * 1000.0);
            let size = meta
                .get("size")
                .and_then(Value::as_f64)
                .unwrap_or(num(&f, "size"))
                .max(0.0) as u64;
            let name = mstr("name");
            files.push(DirFile {
                uuid: id.to_string(),
                name: if name.is_empty() { "(unlesbarer Name)".into() } else { name },
                size,
                mime: mstr("mime"),
                key: mstr("key"),
                modified: (last > 0.0).then_some(last as i64),
                region: f.get("region").and_then(Value::as_str).unwrap_or("").to_string(),
                bucket: f.get("bucket").and_then(Value::as_str).unwrap_or("").to_string(),
                chunks: num(&f, "chunks") as u64,
                version: num(&f, "version") as u64,
            });
        }
        Ok((folders, files))
    }

    /// Folder uuids from the drive root to `path` (first = root). Names match case-insensitively.
    pub async fn resolve_path(&self, s: &Session, path: &str) -> R<Vec<(String, String)>> {
        let mut chain = vec![("/".to_string(), s.base_folder.clone())];
        let mut cur = String::new();
        for seg in path.split('/').filter(|x| !x.trim().is_empty()) {
            let parent = chain.last().unwrap().1.clone();
            let (folders, _) = self.list_dir(s, &parent).await?;
            let f = folders
                .iter()
                .find(|f| f.name == seg)
                .or_else(|| folders.iter().find(|f| f.name.eq_ignore_ascii_case(seg)))
                .ok_or_else(|| format!("Ordner „{seg}“ gibt es in Filen nicht"))?;
            cur = format!("{cur}/{}", f.name);
            chain.push((cur.clone(), f.uuid.clone()));
        }
        Ok(chain)
    }

    // ─────────────────────────── Files ───────────────────────────

    /// Encrypts and uploads a local file into folder `parent` under `name`.
    pub async fn upload_file(
        &self,
        s: &Session,
        path: &Path,
        parent: &str,
        name: &str,
        mime: &str,
        modified_ms: i64,
    ) -> R<Uploaded> {
        let size = tokio::fs::metadata(path)
            .await
            .map_err(|e| format!("Datei fehlt: {e}"))?
            .len();
        if size == 0 {
            return Err("Leere Dateien kann Filen nicht speichern.".into());
        }
        let chunks = size.div_ceil(CHUNK_SIZE as u64);
        let file_uuid = uuid::Uuid::new_v4().to_string();
        let key = random_string(32);
        let upload_key = random_string(32);
        let rm = random_string(32);

        let file = tokio::fs::File::open(path)
            .await
            .map_err(|e| e.to_string())?;
        // Read chunks one after another (also feeding the whole-file hash), upload in parallel.
        let hasher = std::sync::Arc::new(Mutex::new(Sha512::new()));
        let reader = stream::try_unfold(
            (file, 0u64, hasher.clone()),
            |(mut f, index, h)| async move {
                let mut buf = vec![0u8; CHUNK_SIZE];
                let mut filled = 0;
                while filled < CHUNK_SIZE {
                    let n = f
                        .read(&mut buf[filled..])
                        .await
                        .map_err(|e| e.to_string())?;
                    if n == 0 {
                        break;
                    }
                    filled += n;
                }
                if filled == 0 {
                    return Ok::<_, String>(None);
                }
                buf.truncate(filled);
                h.lock().unwrap().update(&buf);
                Ok(Some(((index, buf), (f, index + 1, h))))
            },
        );

        let results: Vec<(String, String)> = reader
            .map_ok(|(index, data)| {
                let key = key.clone();
                let upload_key = upload_key.clone();
                let file_uuid = file_uuid.clone();
                async move {
                    self.upload_chunk(s, &file_uuid, index, parent, &upload_key, &key, &data)
                        .await
                }
            })
            .try_buffer_unordered(UPLOAD_THREADS)
            .try_collect()
            .await?;
        if results.len() as u64 != chunks {
            return Err(format!(
                "Nur {} von {chunks} Teilen hochgeladen – die Datei hat sich während des Hochladens verändert.",
                results.len()
            ));
        }
        let (region, bucket) = results.into_iter().next().unwrap_or_default();
        let hash = hex::encode(hasher.lock().unwrap().clone().finalize());

        let metadata = json!({
            "name": name,
            "size": size,
            "mime": mime,
            "key": key,
            "lastModified": modified_ms,
            "creation": modified_ms,
            "hash": hash,
        });
        self.post(
            s,
            "/v3/upload/done",
            json!({
                "uuid": file_uuid,
                "name": metadata_encrypt(name, &key),
                "nameHashed": name_hash(s, name)?,
                "size": metadata_encrypt(&size.to_string(), &key),
                "chunks": chunks,
                "mime": metadata_encrypt(mime, &key),
                "rm": rm,
                "metadata": metadata_encrypt(&metadata.to_string(), s.current_key()),
                "version": 2,
                "uploadKey": upload_key,
            }),
        )
        .await?;
        Ok(Uploaded {
            uuid: file_uuid,
            key,
            region,
            bucket,
            chunks,
            size,
        })
    }

    #[allow(clippy::too_many_arguments)]
    async fn upload_chunk(
        &self,
        s: &Session,
        uuid: &str,
        index: u64,
        parent: &str,
        upload_key: &str,
        key: &str,
        data: &[u8],
    ) -> R<(String, String)> {
        let encrypted = data_encrypt(data, key)?;
        let hash = sha512_hex(&encrypted);
        let index_s = index.to_string();
        // Same order as the SDK: the Checksum header is the SHA-512 of the JSON of the query parameters.
        let checksum_json = format!(
            "{{\"uuid\":{},\"index\":{},\"parent\":{},\"uploadKey\":{},\"hash\":{}}}",
            Value::from(uuid),
            Value::from(index_s.as_str()),
            Value::from(parent),
            Value::from(upload_key),
            Value::from(hash.as_str())
        );
        let checksum = sha512_hex(checksum_json.as_bytes());
        let mut last = String::new();
        for attempt in 0..CHUNK_RETRIES {
            if attempt > 0 {
                tokio::time::sleep(Duration::from_secs(2u64.pow(attempt))).await;
            }
            let url = format!(
                "{}/v3/upload?uuid={uuid}&index={index_s}&parent={parent}&uploadKey={upload_key}&hash={hash}",
                pick(&INGESTS)
            );
            let res = self
                .http
                .post(&url)
                .bearer_auth(&s.api_key)
                .header("Accept", "application/json")
                .header("Checksum", &checksum)
                .body(encrypted.clone())
                .send()
                .await;
            let res = match res {
                Ok(r) => r,
                Err(e) => {
                    last = format!("Upload von Teil {index} fehlgeschlagen: {e}");
                    continue;
                }
            };
            let status = res.status();
            let text = res.text().await.unwrap_or_default();
            let v: Value = serde_json::from_str(&text).unwrap_or(Value::Null);
            if status.is_success() && v.get("status").and_then(Value::as_bool) == Some(true) {
                let d = v.get("data").cloned().unwrap_or(Value::Null);
                return Ok((
                    d.get("region")
                        .and_then(Value::as_str)
                        .unwrap_or("")
                        .to_string(),
                    d.get("bucket")
                        .and_then(Value::as_str)
                        .unwrap_or("")
                        .to_string(),
                ));
            }
            last = format!(
                "Upload von Teil {index}: HTTP {status} {}",
                v.get("message").and_then(Value::as_str).unwrap_or("")
            );
            if status.is_client_error() && status.as_u16() != 429 && status.as_u16() != 408 {
                break;
            }
        }
        Err(last)
    }

    /// Downloads and decrypts one chunk.
    pub async fn download_chunk(
        &self,
        s: &Session,
        region: &str,
        bucket: &str,
        uuid: &str,
        index: u64,
        key: &str,
    ) -> R<Vec<u8>> {
        let mut last = String::new();
        for attempt in 0..CHUNK_RETRIES {
            if attempt > 0 {
                tokio::time::sleep(Duration::from_secs(2u64.pow(attempt))).await;
            }
            let url = format!("{}/{region}/{bucket}/{uuid}/{index}", pick(&EGESTS));
            match self.http.get(&url).bearer_auth(&s.api_key).send().await {
                Ok(r) if r.status().is_success() => match r.bytes().await {
                    Ok(b) => return data_decrypt(&b, key),
                    Err(e) => last = e.to_string(),
                },
                Ok(r) => {
                    last = format!("Download HTTP {}", r.status());
                    if r.status().as_u16() == 404 {
                        break;
                    }
                }
                Err(e) => last = e.to_string(),
            }
        }
        Err(format!("Teil {index} konnte nicht geladen werden: {last}"))
    }

    /// Deletes a file permanently or moves it to the trash. A file that is already gone counts as deleted.
    pub async fn delete_file(&self, s: &Session, uuid: &str, permanently: bool) -> R<()> {
        let path = if permanently {
            "/v3/file/delete/permanent"
        } else {
            "/v3/file/trash"
        };
        match self.post(s, path, json!({ "uuid": uuid })).await {
            Ok(_) => Ok(()),
            Err(e)
                if e.contains("not found")
                    || e.contains("not_found")
                    || e.contains("does not exist") =>
            {
                Ok(())
            }
            Err(e) => Err(e),
        }
    }
}

impl Session {
    pub fn current_key(&self) -> &str {
        self.master_keys.last().map(String::as_str).unwrap_or("")
    }
}

// ─────────────────────────── Crypto ───────────────────────────

fn pick<'a>(hosts: &[&'a str]) -> &'a str {
    hosts[rand::thread_rng().gen_range(0..hosts.len())]
}

fn str_field(v: &Value, k: &str) -> R<String> {
    v.get(k)
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .ok_or_else(|| format!("Unerwartete Antwort von Filen (\"{k}\" fehlt)"))
}

pub fn random_string(len: usize) -> String {
    let mut rng = rand::thread_rng();
    (0..len)
        .map(|_| CHARSET[rng.gen_range(0..CHARSET.len())] as char)
        .collect()
}

pub fn sha512_hex(data: &[u8]) -> String {
    hex::encode(Sha512::digest(data))
}

/// Login password and master key from the raw password (auth v2: PBKDF2, v3: Argon2id).
pub fn derive_password_and_master_key(
    password: &str,
    salt: &str,
    auth_version: u32,
) -> R<(String, String)> {
    match auth_version {
        2 => {
            let mut out = [0u8; 64];
            pbkdf2::pbkdf2_hmac::<Sha512>(password.as_bytes(), salt.as_bytes(), 200_000, &mut out);
            let derived = hex::encode(out);
            let (master, pw) = derived.split_at(64);
            Ok((master.to_string(), sha512_hex(pw.as_bytes())))
        }
        3 => {
            let salt = hex::decode(salt).map_err(|_| "Ungültiges Salt von Filen".to_string())?;
            let params = argon2::Params::new(65536, 3, 4, Some(64)).map_err(|e| e.to_string())?;
            let a =
                argon2::Argon2::new(argon2::Algorithm::Argon2id, argon2::Version::V0x13, params);
            let mut out = [0u8; 64];
            a.hash_password_into(password.as_bytes(), &salt, &mut out)
                .map_err(|e| e.to_string())?;
            let derived = hex::encode(out);
            let (master, pw) = derived.split_at(64);
            Ok((master.to_string(), pw.to_string()))
        }
        v => Err(format!(
            "Filen-Anmeldeverfahren Version {v} wird nicht unterstützt."
        )),
    }
}

fn metadata_key_v2(key: &str) -> [u8; 32] {
    let mut out = [0u8; 32];
    pbkdf2::pbkdf2_hmac::<Sha512>(key.as_bytes(), key.as_bytes(), 1, &mut out);
    out
}

/// "002" metadata encryption (what the SDK uses by default for every key).
pub fn metadata_encrypt(plain: &str, key: &str) -> String {
    let iv = random_string(12);
    let cipher = Aes256Gcm::new_from_slice(&metadata_key_v2(key)).expect("32 bytes");
    let ct = cipher
        .encrypt(Nonce::from_slice(iv.as_bytes()), plain.as_bytes())
        .expect("encryption cannot fail");
    format!("002{iv}{}", B64.encode(ct))
}

/// Decrypts "002" or "003" metadata, trying every key (newest first).
pub fn metadata_decrypt(data: &str, keys: &[String]) -> R<String> {
    let version = data.get(..3).unwrap_or("");
    for key in keys.iter().rev() {
        let plain = match version {
            "002" => {
                let (Some(iv), Some(rest)) = (data.get(3..15), data.get(15..)) else {
                    break;
                };
                let Ok(ct) = B64.decode(rest) else { break };
                Aes256Gcm::new_from_slice(&metadata_key_v2(key))
                    .ok()
                    .and_then(|c| {
                        c.decrypt(Nonce::from_slice(iv.as_bytes()), ct.as_slice())
                            .ok()
                    })
            }
            "003" => {
                let (Some(iv), Some(rest)) = (data.get(3..27), data.get(27..)) else {
                    break;
                };
                let (Ok(iv), Ok(ct), Ok(k)) = (hex::decode(iv), B64.decode(rest), hex::decode(key))
                else {
                    continue;
                };
                if k.len() != 32 || iv.len() != 12 {
                    continue;
                }
                Aes256Gcm::new_from_slice(&k)
                    .ok()
                    .and_then(|c| c.decrypt(Nonce::from_slice(&iv), ct.as_slice()).ok())
            }
            _ => return Err(format!("Unbekanntes Filen-Metadatenformat \"{version}\"")),
        };
        if let Some(p) = plain {
            return String::from_utf8(p).map_err(|e| e.to_string());
        }
    }
    Err("Filen-Daten konnten nicht entschlüsselt werden (falscher Schlüssel?)".into())
}

/// File data encryption version 2: nonce (12 ASCII chars) ‖ AES-256-GCM(ciphertext ‖ tag).
pub fn data_encrypt(data: &[u8], key: &str) -> R<Vec<u8>> {
    let cipher = Aes256Gcm::new_from_slice(key.as_bytes())
        .map_err(|_| "Dateischlüssel ungültig".to_string())?;
    let iv = random_string(12);
    let ct = cipher
        .encrypt(Nonce::from_slice(iv.as_bytes()), data)
        .map_err(|_| "Verschlüsselung fehlgeschlagen".to_string())?;
    let mut out = Vec::with_capacity(12 + ct.len());
    out.extend_from_slice(iv.as_bytes());
    out.extend_from_slice(&ct);
    Ok(out)
}

pub fn data_decrypt(data: &[u8], key: &str) -> R<Vec<u8>> {
    if data.len() < 12 + 16 {
        return Err("Teil zu kurz".into());
    }
    let k: Vec<u8> = if key.len() == 64 && key.bytes().all(|b| b.is_ascii_hexdigit()) {
        hex::decode(key).map_err(|e| e.to_string())?
    } else {
        key.as_bytes().to_vec()
    };
    let cipher =
        Aes256Gcm::new_from_slice(&k).map_err(|_| "Dateischlüssel ungültig".to_string())?;
    cipher
        .decrypt(Nonce::from_slice(&data[..12]), &data[12..])
        .map_err(|_| "Entschlüsselung fehlgeschlagen (Daten beschädigt?)".into())
}

/// `nameHashed` of files and folders: auth v1/2 SHA-1(SHA-512-hex(name)), v3 HMAC-SHA256 with a
/// key derived from the private key.
pub fn name_hash(s: &Session, name: &str) -> R<String> {
    let lower = name.to_lowercase();
    if s.auth_version < 3 {
        let inner = sha512_hex(lower.as_bytes());
        return Ok(hex::encode(Sha1::digest(inner.as_bytes())));
    }
    let der = B64
        .decode(s.private_key.trim())
        .map_err(|_| "Privater Schlüssel ungültig".to_string())?;
    let hk = hkdf::Hkdf::<Sha256>::new(None, &der);
    let mut key = [0u8; 32];
    hk.expand(b"hmac-sha256-key", &mut key)
        .map_err(|e| e.to_string())?;
    let mut mac = <Hmac<Sha256> as Mac>::new_from_slice(&key).map_err(|e| e.to_string())?;
    mac.update(lower.as_bytes());
    Ok(hex::encode(mac.finalize().into_bytes()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn metadata_roundtrip() {
        let key = random_string(32);
        let enc = metadata_encrypt("{\"name\":\"a\"}", &key);
        assert!(enc.starts_with("002"));
        assert_eq!(
            metadata_decrypt(&enc, &["other".into(), key.clone()]).unwrap(),
            "{\"name\":\"a\"}"
        );
        assert!(metadata_decrypt(&enc, &["x".into()]).is_err());
    }

    #[test]
    fn metadata_003() {
        let key = hex::encode([7u8; 32]);
        let iv = [1u8; 12];
        let c = Aes256Gcm::new_from_slice(&[7u8; 32]).unwrap();
        let ct = c
            .encrypt(Nonce::from_slice(&iv), b"dek".as_slice())
            .unwrap();
        let enc = format!("003{}{}", hex::encode(iv), B64.encode(ct));
        assert_eq!(metadata_decrypt(&enc, &[key]).unwrap(), "dek");
    }

    #[test]
    fn data_roundtrip() {
        let key = random_string(32);
        let enc = data_encrypt(b"hello world", &key).unwrap();
        assert_eq!(enc.len(), 12 + 11 + 16);
        assert_eq!(data_decrypt(&enc, &key).unwrap(), b"hello world");
    }

    #[test]
    fn v2_derivation_shape() {
        let (m, p) = derive_password_and_master_key("pw", "salt", 2).unwrap();
        assert_eq!(m.len(), 64);
        assert_eq!(p.len(), 128);
        // known-answer: PBKDF2-HMAC-SHA512("pw","salt",200000) first 32 bytes, hex
        let mut out = [0u8; 64];
        pbkdf2::pbkdf2_hmac::<Sha512>(b"pw", b"salt", 200_000, &mut out);
        assert_eq!(m, hex::encode(&out[..32]));
    }

    #[test]
    fn name_hash_v2() {
        let s = Session {
            email: String::new(),
            auth_version: 2,
            api_key: String::new(),
            master_keys: vec![],
            private_key: String::new(),
            base_folder: String::new(),
        };
        // sha1(sha512hex("abc"))
        let inner = sha512_hex(b"abc");
        assert_eq!(
            name_hash(&s, "ABC").unwrap(),
            hex::encode(Sha1::digest(inner.as_bytes()))
        );
    }

    /// Known answers produced with the official @filen/sdk (node).
    #[test]
    fn official_sdk_known_answers() {
        let (m, p) = derive_password_and_master_key("Passw0rt!", "someSaltValue123", 2).unwrap();
        assert_eq!(
            m,
            "5e41e78ed517e424f953ce6ad7c9d7170177642d744b6fa1863580b004c7c0d0"
        );
        assert_eq!(
            p,
            "6a40741eb3c307b17f1beecc2ebfba10f6a7b3aadc3bab436b18d5470e46207569398dbbac31d0de15b78e50f687b21cd59c223dd9e472828eedcfd75425410e"
        );
        let (m, p) = derive_password_and_master_key(
            "Passw0rt!",
            "a1b2c3d4e5f60718293a4b5c6d7e8f90a1b2c3d4e5f60718293a4b5c6d7e8f90",
            3,
        )
        .unwrap();
        assert_eq!(
            m,
            "9b84dd8a414041159dc9246e9403f040a8f33db719a9f63288daefe0fbdad0fb"
        );
        assert_eq!(
            p,
            "c815c7d11a11f787a9b2005e43cca839fd6a9215768b9e0f1459a57a0e3b2295"
        );

        let key = "masterkeyABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789abcdefghijklmnopqrstu".to_string();
        let meta = "002ga3iBKpv1a7EdIBMUWhR890AQ+QYs4OOeJBI5j52dGRkXVdHLIquFG6X/zqK";
        assert_eq!(
            metadata_decrypt(meta, &[key]).unwrap(),
            "{\"name\":\"nrzBackup\"}"
        );

        let data = B64
            .decode("LWl3M1ZKVHdRVElkGFHxuHTTj2KmITi/22pbK3bXTrY2k7+ojGC2oL+eucfF")
            .unwrap();
        assert_eq!(
            data_decrypt(&data, "abcdefghijklmnopqrstuvwxyz012345").unwrap(),
            b"hello filen chunk"
        );

        let mut s = Session {
            email: String::new(),
            auth_version: 2,
            api_key: String::new(),
            master_keys: vec![],
            private_key: "dGhpcy1pcy1ub3QtYS1yZWFsLWRlci1wcml2YXRlLWtleS1idXQtYnl0ZXM=".into(),
            base_folder: String::new(),
        };
        assert_eq!(
            name_hash(&s, "NrzBackup").unwrap(),
            "a8444c8d8d0e1d3d59cec90a4f0792f31b2f1715"
        );
        s.auth_version = 3;
        assert_eq!(
            name_hash(&s, "NrzBackup").unwrap(),
            "814808a62e0a2becc9004c64b0cd99147cf263de5cb05ad7c357cc1781d81463"
        );
    }
}
