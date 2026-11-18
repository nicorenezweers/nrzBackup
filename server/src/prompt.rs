//! "KI-Anleitung": a ready-to-paste prompt for a coding agent that integrates nrzBackup
//! into an app – push, being asked for a backup, restore, settings UI and tests.
//! Contains every address and setting of the source, but never a token.

use nrzbackup_shared::*;

use crate::db::Source;

fn schedule_text(s: &Schedule) -> String {
    match s.mode {
        ScheduleMode::Off => {
            "kein Zeitplan (nur auf Knopfdruck in nrzBackup oder per MCP `request_backup`)".into()
        }
        ScheduleMode::Daily => format!("täglich um {} Uhr (Zeitzone des Servers)", s.time),
        ScheduleMode::Interval => format!("alle {} Stunden", s.every_hours.max(1)),
    }
}

fn retention_text(r: &Retention) -> String {
    format!(
        "neueste {}, täglich {}, wöchentlich {}, monatlich {}, jährlich {}",
        r.keep_last, r.daily, r.weekly, r.monthly, r.yearly
    )
}

pub fn build(s: &Source, public_url: &str, root_folder: &str) -> String {
    let push_url = format!("{public_url}/api/v1/backup");
    let mcp_url = format!("{public_url}/mcp");
    let status_url = format!("{public_url}/api/v1/status");
    let tool = if s.pull_tool.trim().is_empty() {
        "create_backup"
    } else {
        s.pull_tool.trim()
    };
    let expect = if s.expect_hours > 0 {
        format!(
            "nrzBackup erwartet **mindestens alle {} Stunden** ein Backup. Bleibt es länger aus, wird die Quelle gelb, ab dem Doppelten rot.",
            s.expect_hours
        )
    } else {
        "In nrzBackup ist kein fester Takt hinterlegt; sinnvoll ist trotzdem mindestens ein Backup pro Tag.".into()
    };
    let description = if s.description.trim().is_empty() {
        String::new()
    } else {
        format!("\nBeschreibung der Quelle: {}\n", s.description.trim())
    };

    let pull_now = match s.pull_kind {
        PullKind::Mcp => format!(
            "In nrzBackup ist für diese Quelle **Anfordern per MCP** eingestellt:\n\n\
             - MCP-URL der App (so ruft nrzBackup sie auf): `{url}`\n\
             - Tool: `{tool}`\n\
             - Argumente, die nrzBackup schickt: `{args}`\n\
             - Zeitplan: {sched}\n\
             - Authentifizierung: `Authorization: Bearer <Token der App>` – diesen Token erzeugt/verwaltet **die App** \
             (z. B. in ihren Einstellungen) und der Nutzer trägt ihn in nrzBackup bei der Quelle ein.\n\n\
             Passe Tool-Name und URL unten an diese Werte an (oder melde, wenn du andere Werte brauchst – dann \
             trägt der Nutzer sie in nrzBackup um).",
            url = if s.pull_url.is_empty() { "(noch leer)" } else { &s.pull_url },
            args = if s.pull_args.trim().is_empty() { "{}" } else { s.pull_args.trim() },
            sched = schedule_text(&s.schedule),
        ),
        PullKind::Http => format!(
            "In nrzBackup ist für diese Quelle **Anfordern per HTTP** eingestellt:\n\n\
             - URL: `{} {}` – die Antwort muss die Backup-Datei selbst sein\n\
             - Zeitplan: {}\n\
             - Authentifizierung: `Authorization: Bearer <Token der App>` – diesen Token erzeugt/verwaltet **die App**, \
             der Nutzer trägt ihn in nrzBackup ein.\n\n\
             Baue diesen HTTP-Endpunkt (Variante B unten). Zusätzlich das MCP-Tool zu bauen schadet nicht.",
            s.pull_method,
            if s.pull_url.is_empty() { "(noch leer)" } else { &s.pull_url },
            schedule_text(&s.schedule),
        ),
        PullKind::Umbrel => format!(
            "nrzBackup sichert diese Quelle bereits **direkt über umbrelOS**: es lädt `{}` als ZIP herunter{} \
             ({}). Eine eigene Integration ist trotzdem sinnvoll, wenn die App einen sauberen Export braucht \
             (z. B. Datenbank-Dump statt laufender Dateien). Baue dafür das MCP-Tool `create_backup` (Variante A); \
             danach kann der Nutzer die Quelle in nrzBackup auf „Anfordern: MCP“ umstellen.",
            s.umbrel_path,
            if s.stop_app { " und hält die App dafür kurz an" } else { "" },
            schedule_text(&s.schedule),
        ),
        PullKind::None => "In nrzBackup ist für diese Quelle **noch kein Anfordern** eingestellt. Baue trotzdem das \
             MCP-Tool `create_backup` (Variante A) – danach trägt der Nutzer in nrzBackup bei der Quelle \
             „Anfordern: MCP“, die MCP-URL der App, den Token der App und einen Zeitplan ein."
            .into(),
    };

    format!(
        r#"# Aufgabe: nrzBackup in diese App integrieren (Quelle „{name}“)

Du arbeitest im Code einer App, die auf umbrelOS läuft. Baue eine vollständige, robuste Anbindung an **nrzBackup**.
nrzBackup nimmt Backup-Dateien entgegen, verschlüsselt sie auf dem Gerät Ende-zu-Ende (AES-256-GCM) und lädt sie zu
Filen.io hoch. Es hebt pro Quelle nur so viele Kopien auf wie eingestellt und zeigt in nrzDashboard eine Ampel.
Backups lassen sich **von beiden Seiten** auslösen: die App sendet selbst (Push), oder nrzBackup fordert an (Pull).
Beides soll funktionieren.

## Daten dieser Quelle

| | |
|---|---|
| Name in nrzBackup | {name} |
| Kennung (slug) | `{slug}` |
| Ordner in Filen | `/{root}/{slug}/` |
| Basis-URL von nrzBackup (aus anderen Umbrel-Apps erreichbar) | `{public_url}` |
| Push-Endpunkt | `POST {push_url}?name=<dateiname>` |
| Status-Endpunkt | `GET {status_url}` |
| MCP-Server von nrzBackup | `{mcp_url}` |
| Aufbewahrung in nrzBackup | {retention} |
{description}
{expect}

### Token – wichtig
Der **Push-Token** dieser Quelle beginnt mit `nrzbk_` und wird hier absichtlich **nicht** mitgeliefert. Er darf nie im
Code, in Logs, in Fehlermeldungen oder im Git-Repo landen. Lies ihn zur Laufzeit aus einer Einstellung der App
(verschlüsselt/geheim gespeichert, in der Oberfläche nur maskiert angezeigt) oder aus der Umgebungsvariable
`NRZBACKUP_TOKEN`. Die Basis-URL ist ebenfalls einstellbar: Einstellung oder `NRZBACKUP_URL`, Standard `{public_url}`.

## 1. Was ins Backup gehört

- Alles, was nötig ist, um die App auf einem neuen Gerät vollständig wiederherzustellen: Datenbank, hochgeladene
  Dateien, Konfiguration/Einstellungen. Keine Caches, keine temporären Dateien, keine Programmdateien.
- **Konsistenter Schnappschuss**: SQLite per Backup-API oder `VACUUM INTO '<tmp>'` (nie die laufende Datei kopieren),
  PostgreSQL per `pg_dump`, Dateien danach zusammen in **ein** Archiv (`.tar.gz` oder `.zip`), mit einer kleinen
  `manifest.json` (App, App-Version, Schema-Version, Zeitpunkt, Inhaltsliste).
- Dateiname mit App und Zeitpunkt, z. B. `{slug}_2026-11-12_0300.tar.gz`. Leere Dateien nimmt nrzBackup nicht an.
- Das Backup wird von nrzBackup verschlüsselt; die App muss nicht selbst verschlüsseln.
- Temporäre Dateien nach dem Senden löschen. Große Backups streamen, nicht komplett in den Speicher laden.

## 2. Push: die App sendet selbst (Pflicht)

```
POST {push_url}?name={slug}_2026-11-12_0300.tar.gz
Authorization: Bearer <NRZBACKUP_TOKEN>
Content-Type: application/octet-stream

<rohe Bytes der Datei>
```

Antworten:
- `201` → angenommen und auf dem Gerät gesichert (fsync); Body: `{{"id", "filename", "size", "sha256", "status": "queued", "message"}}`.
  Ab hier ist das Backup sicher – nrzBackup lädt es selbst zu Filen und versucht es bei Fehlern immer wieder.
- `400` → z. B. leere Datei (Body `{{"error": "..."}}`) – nicht wiederholen, Fehler anzeigen.
- `401` → Token fehlt/falsch – nicht wiederholen, in der App „Token prüfen“ anzeigen.
- Netzwerkfehler/`5xx` → mit Backoff wiederholen (z. B. 1, 5, 15, 60 Minuten), das Archiv bis dahin behalten.

Beispiel zum Testen:
```bash
curl -X POST -H "Authorization: Bearer $NRZBACKUP_TOKEN" --data-binary @backup.tar.gz \
  "{push_url}?name=backup.tar.gz"
```

Zeitplan in der App: automatisch mindestens im erwarteten Takt (siehe oben), zusätzlich ein Knopf „Jetzt sichern“.
Prüfe optional nach dem Senden die SHA-256 aus der Antwort gegen die eigene.

Alternativ per MCP an `{mcp_url}` (gleicher Bearer-Token): Tool `backup_file` mit `{{"filename", "content_base64"}}`
(nur kleine Dateien) oder `backup_from_url` mit `{{"url", "filename", "headers"}}` (nrzBackup lädt die Datei selbst).
HTTP-Push ist für Dateien jeder Größe die bessere Wahl.

## 3. Pull: nrzBackup fordert ein Backup an

{pull_now}

### Variante A – MCP-Tool (empfohlen)
Die App stellt einen MCP-Server bereit (JSON-RPC 2.0 über Streamable HTTP, `POST /mcp`; `initialize`, `ping`,
`tools/list`, `tools/call`; zustandslos reicht). Er prüft `Authorization: Bearer <Token der App>`.
Tool `{tool}`: erstellt ein frisches Backup und gibt es zurück. Erlaubte Antworten (als `structuredContent` **und** als
JSON-Text in `content[0].text`):

1. Datei direkt (bis ca. 100 MB): `{{"filename": "...", "content_base64": "..."}}`
2. Download-Link (beliebig groß): `{{"filename": "...", "url": "http://<app-container>:<port>/backup/<einmal-id>"}}` –
   nrzBackup lädt sofort herunter; liegt der Link auf demselben Host wie die MCP-URL, schickt nrzBackup den
   Bearer-Token automatisch mit, sonst `"headers": {{"Authorization": "..."}}` mitgeben. Link nur kurz gültig machen.
3. Später liefern (sehr große/langsame Backups): sofort mit Text antworten, z. B. „Backup wird erstellt“, und die Datei
   danach per Push senden. Dafür trägt der Nutzer in nrzBackup bei den Argumenten
   `{{"push_url": "{{{{push_url}}}}", "push_token": "{{{{push_token}}}}"}}` ein; nrzBackup ersetzt die Platzhalter durch die
   Push-URL und einen **Einmal-Token** (24 h gültig). Dann mit genau diesen Werten pushen.

Fehler als Tool-Ergebnis mit `"isError": true` und verständlichem Text melden.

### Variante B – HTTP-Endpunkt
`GET` (oder `POST`) auf einen Endpunkt der App, geschützt mit `Authorization: Bearer <Token der App>`; Antwort `200`
mit der Datei als Body und `Content-Disposition: attachment; filename="..."`.

## 4. Wiederherstellen

- `GET {status_url}` mit dem Push-Token: Ampel dieser Quelle (`health`: ok/warning/error/unknown, `health_message`,
  `copies`, `last_backup`).
- MCP-Tool `list_backups` (Push-Token): Kopien mit `id`, `filename`, `created_at`, `size`, `sha256`, `status`.
- `GET {public_url}/api/v1/backups/<id>/download` mit dem Push-Token: liefert die entschlüsselte Datei.
- Baue in der App „Aus nrzBackup wiederherstellen“: Kopie auswählen, herunterladen, Manifest prüfen (App/Schema),
  vorher den aktuellen Stand sichern, dann einspielen.

## 5. Oberfläche in der App

Abschnitt „Backup (nrzBackup)“ in den Einstellungen: nrzBackup-URL, Push-Token (maskiert), eigener Token für das
Anfordern (erzeugen/anzeigen/neu erzeugen), Zeitplan, Knopf „Jetzt sichern“, letzter Versuch mit Ergebnis, Ampel aus
dem Status-Endpunkt, Wiederherstellen.

## 6. Fertig, wenn

- Ein Klick auf „Jetzt sichern“ ein vollständiges Archiv erzeugt und nrzBackup mit `201` antwortet.
- Der automatische Zeitplan läuft und Fehler mit Backoff wiederholt werden.
- `tools/call {tool}` (bzw. der HTTP-Endpunkt) ein frisches Backup liefert und ohne/mit falschem Token `401` kommt.
- Ein Backup über `/api/v1/backups/<id>/download` wieder eingespielt werden kann.
- Kein Token im Code, in Logs oder im Repo steht.
- Tests für Archiv-Erstellung, Push (inkl. 401/400/Netzwerkfehler) und das MCP-Tool existieren.
"#,
        name = s.name,
        slug = s.slug,
        root = root_folder,
        retention = retention_text(&s.retention),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn no_token_and_placeholders_escaped() {
        let s = Source {
            id: 1,
            slug: "nrzbudget".into(),
            name: "nrzBudget".into(),
            description: String::new(),
            token_hint: "nrzbk_abcd…".into(),
            retention: Retention::default(),
            expect_hours: 26,
            pull_kind: PullKind::Mcp,
            pull_url: "http://nrz-budget_app_1:8080/mcp".into(),
            pull_token: "v1:SECRET".into(),
            pull_method: "GET".into(),
            pull_tool: "create_backup".into(),
            pull_args: String::new(),
            schedule: Schedule::default(),
            last_pull: None,
            next_pull: None,
            pull_error: None,
            created_at: 0,
            umbrel_path: String::new(),
            umbrel_app: String::new(),
            stop_app: false,
        };
        let t = build(&s, "http://nrz-backup_app_1:8080", "nrzBackup");
        assert!(!t.contains("SECRET") && !t.contains("nrzbk_abcd"));
        assert!(t.contains("{{push_token}}"));
        assert!(t.contains("POST http://nrz-backup_app_1:8080/api/v1/backup?name="));
        assert!(t.contains("26 Stunden"));
    }
}
