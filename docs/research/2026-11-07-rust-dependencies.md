# Rust-Abhängigkeiten: Updates und Sicherheitsstand

Recherche für [Rust-Abhängigkeiten: Updates und Sicherheitsstand ermitteln](https://github.com/micschr0/gluco-hub-rs/issues/120), Teil von [Technischen Stack modernisieren und bessere Libraries bewerten](https://github.com/micschr0/gluco-hub-rs/issues/119). Abruf: **2026-11-07 UTC**. Recherche-Basis: [`428b9192f8a623356a2ad7c436f9e3d4dbfa15b1`](https://github.com/micschr0/gluco-hub-rs/tree/428b9192f8a623356a2ad7c436f9e3d4dbfa15b1). Planung; keine Abhängigkeit geändert und keine Upgrade-Freigabe.

## Entscheidungsvorschlag

Den bestehenden Rust-Stack grundsätzlich behalten. Von 36 direkten externen Crates sind 34 auf dem höchsten stabilen, nicht zurückgezogenen Release. `uuid` benötigt nur ein kompatibles Lockfile-Update; `testcontainers` 0.28 benötigt eine eigene, koordinierte Entscheidung. **Vorrang hat die Bereinigung der MQTT-TLS-Ausnahmen und der inzwischen erweiterten RustSec-Befunde.** Gleiche Versionsstände bedeuten keine Sicherheitsentwarnung.

Der vollständige Namens-/Versionsabgleich des Lockfiles findet acht einschlägige RustSec-Einträge: fünf für `rustls-webpki 0.102.8`, drei Wartungshinweise. Drei davon fehlen in den bestehenden Ausnahmen. Die betroffenen Versionsbereiche sind belegt; tatsächliche Laufzeit-Erreichbarkeit und Exploit-Voraussetzungen sind damit nicht bewiesen. Quellen und Grenzen folgen unten.

## Methode und Reichweite

- Alle drei Cargo-Manifeste und das unveränderte Lockfile maschinell als TOML gelesen: **36 externe direkte Crates**, zwei interne Workspace-Pakete, **393 externe Lockfile-Pakete** (395 insgesamt). Workspace-Vererbung, Dev-Abhängigkeiten und optionale Features einbezogen. Die interne Pfadabhängigkeit `gluco-hub-core` ist durch denselben Commit fixiert und hat keinen externen Aktualisierungskandidaten. [Manifeste und Lockfile der Basis](https://github.com/micschr0/gluco-hub-rs/tree/428b9192f8a623356a2ad7c436f9e3d4dbfa15b1).
- Für alle direkten Crates sowie zwölf ausgewählte transitive HTTP-/TLS-/Container-Crates die offizielle crates.io-API abgefragt. „Aktuell“ bedeutet höchste SemVer ohne Prerelease, nicht yanked, Veröffentlichungsdatum spätestens 2026-11-07. Nach Versionsnummer sortiert: ein später veröffentlichtes `sha1 0.10.7` ersetzt nicht `0.11.0`. Jede Tabellenzeile verlinkt ihren Registry-Datensatz.
- Den offiziellen [RustSec-Snapshot `309392b18f6b17022f82c701f300433126901068`](https://github.com/RustSec/advisory-db/tree/309392b18f6b17022f82c701f300433126901068) vom 2026-11-06 vollständig geladen. Alle 393 externen Lock-Pakete nach Namen abgeglichen; 120 nicht zurückgezogene Advisory-Kandidaten anhand `patched`/`unaffected` ausgewertet. Das ist eine nachvollziehbare Ersatzrecherche, **kein erfolgreicher cargo-deny-/cargo-audit-Lauf** und keine umfassende Sicherheitsprüfung.
- Die von Cargo aktualisierte lokale offizielle Sparse-Registry enthält alle 393 gesperrten Versionen: keine als yanked markiert; kein deklarierter `rust_version` oberhalb 1.95. Bei 78 Paketen fehlt diese Angabe. Lizenzdetails aller transitiven Quellen wurden nicht vollständig gescannt. Registry-Metadaten sind keine Build- oder Lizenzfreigabe.

## Vollständiges direktes Inventar

MSRV ist die Angabe des aktuellen Releases in der Registry, keine im Projekt getestete Untergrenze. `k. A.` bedeutet fehlende Metadaten. Lizenzkürzel: M = MIT, A = Apache-2.0, B3 = BSD-3-Clause, I = ISC; `/` bedeutet alternative Lizenzen, `+` beide Bedingungen. Alle angegebenen direkten Lizenzkennungen sind in der bestehenden [deny.toml-Policy](https://github.com/micschr0/gluco-hub-rs/blob/428b9192f8a623356a2ad7c436f9e3d4dbfa15b1/deny.toml) enthalten; Erfüllung der konkreten Lizenzpflichten wurde nicht geprüft.

| Crate / Primärquelle | Deklariert | Direkt gesperrt | Aktuell stabil | MSRV | Einsatz; Lizenz |
|---|---|---|---|---|---|
| [serde](https://crates.io/api/v1/crates/serde) | `1` | 1.0.229 | 1.0.229 | 1.56 | Laufzeit; M/A |
| [serde_json](https://crates.io/api/v1/crates/serde_json) | `1` | 1.0.154 | 1.0.154 | 1.71 | Laufzeit; M/A |
| [chrono](https://crates.io/api/v1/crates/chrono) | `0.4` | 0.4.45 | 0.4.45 | 1.62.0 | Laufzeit; M/A |
| [chrono-tz](https://crates.io/api/v1/crates/chrono-tz) | `0.10` | 0.10.4 | 0.10.4 | 1.65 | Laufzeit; M/A |
| [thiserror](https://crates.io/api/v1/crates/thiserror) | `2` | 2.0.21 | 2.0.21 | 1.77 | Laufzeit; M/A |
| [anyhow](https://crates.io/api/v1/crates/anyhow) | `1` | 1.0.104 | 1.0.104 | 1.68 | Laufzeit; M/A |
| [tokio](https://crates.io/api/v1/crates/tokio) | `1` | 1.54.1 | 1.54.1 | 1.85 | Laufzeit; M |
| [async-trait](https://crates.io/api/v1/crates/async-trait) | `0.1` | 0.1.92 | 0.1.92 | 1.71 | Laufzeit; M/A |
| [futures](https://crates.io/api/v1/crates/futures) | `0.3` | 0.3.34 | 0.3.34 | 1.71 | Laufzeit; M/A |
| [axum](https://crates.io/api/v1/crates/axum) | `0.8` | 0.8.9 | 0.8.9 | 1.80 | Laufzeit; M |
| [reqwest](https://crates.io/api/v1/crates/reqwest) | `0.13` | 0.13.5 | 0.13.5 | 1.85.0 | Laufzeit; M/A |
| [tower](https://crates.io/api/v1/crates/tower) | `0.5` | 0.5.3 | 0.5.3 | 1.64.0 | Laufzeit; M |
| [tower-http](https://crates.io/api/v1/crates/tower-http) | `0.7` | 0.7.1 | 0.7.1 | 1.65 | Laufzeit; M |
| [tracing](https://crates.io/api/v1/crates/tracing) | `0.1` | 0.1.44 | 0.1.44 | 1.65.0 | Laufzeit; M |
| [tracing-subscriber](https://crates.io/api/v1/crates/tracing-subscriber) | `0.3` | 0.3.23 | 0.3.23 | 1.65.0 | Laufzeit; M |
| [metrics](https://crates.io/api/v1/crates/metrics) | `0.24` | 0.24.6 | 0.24.6 | 1.71.1 | Laufzeit; M |
| [metrics-exporter-prometheus](https://crates.io/api/v1/crates/metrics-exporter-prometheus) | `0.18` | 0.18.3 | 0.18.3 | 1.71.1 | Laufzeit; M+A |
| [config](https://crates.io/api/v1/crates/config) | `0.15` | 0.15.27 | 0.15.27 | 1.88.0 | Laufzeit; M/A |
| [secrecy](https://crates.io/api/v1/crates/secrecy) | `0.10` | 0.10.3 | 0.10.3 | 1.60 | Laufzeit; M/A |
| [validator](https://crates.io/api/v1/crates/validator) | `0.21` | 0.21.0 | 0.21.0 | 1.88 | Laufzeit; M |
| [clap](https://crates.io/api/v1/crates/clap) | `4` | 4.6.7 | 4.6.7 | 1.85 | Laufzeit; M/A |
| [tempfile](https://crates.io/api/v1/crates/tempfile) | `3` | 3.27.0 | 3.27.0 | 1.63 | Laufzeit; M/A |
| [subtle](https://crates.io/api/v1/crates/subtle) | `2` | 2.6.1 | 2.6.1 | k. A. | Laufzeit; B3 |
| [sha2](https://crates.io/api/v1/crates/sha2) | `0.11` | 0.11.1 | 0.11.1 | 1.88 | LLU optional; M/A |
| [sha1](https://crates.io/api/v1/crates/sha1) | `0.11` | 0.11.0 | 0.11.0 | 1.85 | NS optional; M/A |
| [base64](https://crates.io/api/v1/crates/base64) | `0.23` | 0.23.1 | 0.23.1 | 1.71.0 | Laufzeit; M/A |
| [rumqttc](https://crates.io/api/v1/crates/rumqttc) | `0.25` | 0.25.1 | 0.25.1 | 1.64.0 | MQTT optional; A |
| [bytes](https://crates.io/api/v1/crates/bytes) | `1` | 1.12.1 | 1.12.1 | 1.57 | MQTT optional; M |
| [tokio-util](https://crates.io/api/v1/crates/tokio-util) | `0.7` | 0.7.20 | 0.7.20 | 1.85 | MQTT optional; M |
| [rustls-pemfile](https://crates.io/api/v1/crates/rustls-pemfile) | `2` | 2.2.0 | 2.2.0 | k. A. | MQTT optional; A/I/M |
| [rustls-native-certs](https://crates.io/api/v1/crates/rustls-native-certs) | `0.8` | 0.8.5 | 0.8.5 | 1.71 | MQTT optional; A/I/M |
| [wiremock](https://crates.io/api/v1/crates/wiremock) | `0.6` | 0.6.5 | 0.6.5 | k. A. | Test; M/A |
| [http-body-util](https://crates.io/api/v1/crates/http-body-util) | `0.1` | 0.1.5 | 0.1.5 | 1.61 | Test; M |
| [testcontainers](https://crates.io/api/v1/crates/testcontainers) | `0.27` | 0.27.3 | 0.28.0 | 1.88 | Test; M/A |
| [testcontainers-modules](https://crates.io/api/v1/crates/testcontainers-modules) | `0.15` | 0.15.0 | 0.15.0 | 1.88 | Test; M |
| [uuid](https://crates.io/api/v1/crates/uuid) | `1` | 1.29.0 | 1.29.1 | 1.89.0 | Test; M/A |

`base64 0.22.1` und `tower-http 0.6.11` kommen zusätzlich transitiv vor; die Tabelle zeigt jeweils die direkt ausgewählte Version. Quellbeleg: [Cargo.lock](https://github.com/micschr0/gluco-hub-rs/blob/428b9192f8a623356a2ad7c436f9e3d4dbfa15b1/Cargo.lock).

## Update-Klassen und Breaking Changes

| Klasse | Befund | Empfehlung / Grenze |
|---|---|---|
| Kompatibles Lockfile-Update | `uuid 1.29.0` auf `1.29.1` liegt im vorhandenen Bereich `1`. | Renovates Lockfile-Wartung verwenden, keine Manifest-Anpassung nötig. [Registry](https://crates.io/api/v1/crates/uuid). |
| Manifest-/Kompatibilitätswechsel | `testcontainers 0.27.3` auf `0.28.0` verlässt den Bereich `0.27`; auch 0.x-Minor-Sprünge können inkompatibel sein. | Vorhandene [Renovate-PR „update rust crate testcontainers to 0.28“](https://github.com/micschr0/gluco-hub-rs/pull/86) reparieren, keine doppelte Upgrade-PR. [Cargo-Kompatibilitätsregeln](https://doc.rust-lang.org/cargo/reference/specifying-dependencies.html#version-requirement-syntax). |
| Gekoppeltes Test-Ökosystem | `testcontainers-modules 0.15.0` fordert `testcontainers ^0.27.0`. Ein alleiniger 0.28-Bump kann zwei Versionen mit verschiedenen öffentlichen Typen erzeugen. | Beiden Crates zusammen eine kompatible Zukunft geben oder konkret verwendete Images lokal definieren; Aufwand und Nutzen erst separat prüfen. Kein erfolgreicher 0.28-Kompilierungstest behauptet. [Offizielle Abhängigkeitsdaten](https://crates.io/api/v1/crates/testcontainers-modules/0.15.0/dependencies). |
| Transitive Container-Wechsel | 0.28 wechselt u. a. Bollard 0.20 auf 0.21; der neue Stand hängt weiterhin an `astral-tokio-tar ^0.6.1`. | 0.28 beseitigt nicht automatisch den Wartungshinweis für Tar. Docker-API-/Compose-Verhalten bei späterer Umsetzung gezielt prüfen. [Release-Changelog](https://github.com/testcontainers/testcontainers-rs/blob/0.28.0/CHANGELOG.md), [Abhängigkeitsdaten 0.28](https://crates.io/api/v1/crates/testcontainers/0.28.0/dependencies). |
| Sicherheitsbedingte Manifest-/Upstream-Arbeit | `rumqttc 0.25.1` fordert direkt `rustls-webpki ^0.102.8` und `rustls-pemfile ^2.2.0`; 0.103-WebPKI passt nicht hinein. | Kein `cargo update` allein kann die WebPKI-Nebenlinie ersetzen. Upstream-Änderung, eng begrenzter Fork oder Wechsel erst nach Nutzen-/Wartungsabwägung; keine pauschale MQTT-Bibliotheksmigration. [Offizielle Abhängigkeitsdaten](https://crates.io/api/v1/crates/rumqttc/0.25.1/dependencies). |

Beim Abruf ist die Testcontainers-PR offen und im Dashboard als manuell bearbeitet/blockiert geführt. Ihr Kopf `71f3872f34d1aaff21e7226a9a1e4bd30842652a` ändert nur das Manifest; mehrere CI-Prüfungen einschließlich Clippy, Tests, MSRV und cargo-deny sind fehlgeschlagen (Checks vom 2026-08-29). Diese historischen Ergebnisse beweisen keinen aktuellen Fehlergrund und keine Upgrade-Eignung. [PR und Checks](https://github.com/micschr0/gluco-hub-rs/pull/86/checks), [Dependency Dashboard](https://github.com/micschr0/gluco-hub-rs/issues/67).

## Transitive Versionen und TLS-Grenzen

Die gezielt aktuell abgefragten `rustls 0.23.46`, `tokio-rustls 0.26.6`, `rustls-pki-types 1.15.1`, `hyper 1.12.0`, `h2 0.4.20`, `ring 0.17.14`, `aws-lc-rs 1.19.0`, `time 0.3.55`, `url 2.5.8` und `webpki-roots 1.0.11` stehen bereits auf der höchsten stabilen Version. Ausnahmen: `rustls-webpki` ist parallel auf 0.102.8 und 0.103.16 gesperrt; `bollard 0.20.2` liegt hinter 0.21.1 und hängt am Testcontainers-Wechsel. Es wurden nicht alle transitiven Crates auf den jeweils neuesten Release untersucht. Quellen: [Lockfile](https://github.com/micschr0/gluco-hub-rs/blob/428b9192f8a623356a2ad7c436f9e3d4dbfa15b1/Cargo.lock), [rustls](https://crates.io/api/v1/crates/rustls), [tokio-rustls](https://crates.io/api/v1/crates/tokio-rustls), [PKI-Typen](https://crates.io/api/v1/crates/rustls-pki-types), [hyper](https://crates.io/api/v1/crates/hyper), [h2](https://crates.io/api/v1/crates/h2), [ring](https://crates.io/api/v1/crates/ring), [aws-lc-rs](https://crates.io/api/v1/crates/aws-lc-rs), [time](https://crates.io/api/v1/crates/time), [url](https://crates.io/api/v1/crates/url), [Roots](https://crates.io/api/v1/crates/webpki-roots), [WebPKI](https://crates.io/api/v1/crates/rustls-webpki), [Bollard](https://crates.io/api/v1/crates/bollard).

Der Lockgraph ist präziser als die vorhandenen Waiver-Kommentare: `rumqttc` hängt **direkt** an WebPKI 0.102.8; `tokio-rustls 0.26.6` hängt an `rustls 0.23.46`, das bereits WebPKI 0.103.16 nutzt. Eine noch nicht stabile Rustls-0.24-Version ist für diesen Teilpfad nicht erforderlich. Das bloße Vorkommen der alten WebPKI-Crate beweist allerdings nicht, welche ihrer Funktionen im konkreten MQTT-TLS-Pfad ausgeführt werden. [Lockgraph](https://github.com/micschr0/gluco-hub-rs/blob/428b9192f8a623356a2ad7c436f9e3d4dbfa15b1/Cargo.lock), [rumqttc-Anforderungen](https://crates.io/api/v1/crates/rumqttc/0.25.1/dependencies).

## RustSec-Befunde im gesperrten Graphen

„Ignoriert“ beschreibt die bestehende Policy, keine Bestätigung ihrer Risikoannahmen. Die folgenden Versionsmatches stammen aus dem oben fixierten RustSec-Snapshot; die Links führen zu seinen unveränderlichen Original-Advisories.

| Advisory / Primärquelle | Gesperrte Crate | Inhalt und Fix-/Wartungsstand | Bestehende Ausnahme |
|---|---|---|---|
| [RUSTSEC-2026-0049](https://github.com/RustSec/advisory-db/blob/309392b18f6b17022f82c701f300433126901068/crates/rustls-webpki/RUSTSEC-2026-0049.md) | WebPKI 0.102.8 | CRL-/DistributionPoint-Verarbeitung; gepatcht ab 0.103.10. | Ja |
| [RUSTSEC-2026-0098](https://github.com/RustSec/advisory-db/blob/309392b18f6b17022f82c701f300433126901068/crates/rustls-webpki/RUSTSEC-2026-0098.md) | WebPKI 0.102.8 | URI-Namensbedingungen; stabil gepatcht ab 0.103.12. | Ja |
| [RUSTSEC-2026-0099](https://github.com/RustSec/advisory-db/blob/309392b18f6b17022f82c701f300433126901068/crates/rustls-webpki/RUSTSEC-2026-0099.md) | WebPKI 0.102.8 | DNS-/Wildcard-Namensbedingungen; stabil gepatcht ab 0.103.12. | Ja |
| [RUSTSEC-2026-0104](https://github.com/RustSec/advisory-db/blob/309392b18f6b17022f82c701f300433126901068/crates/rustls-webpki/RUSTSEC-2026-0104.md) | WebPKI 0.102.8 | Panic beim CRL-Parsing; stabil gepatcht ab 0.103.13. | Ja |
| [RUSTSEC-2026-0382](https://github.com/RustSec/advisory-db/blob/309392b18f6b17022f82c701f300433126901068/crates/rustls-webpki/RUSTSEC-2026-0382.md) | WebPKI 0.102.8 | Zertifikatspfade können distrusted emSign-Zwischenschlüssel akzeptieren; stabil gepatcht ab 0.103.16. | **Nein** |
| [RUSTSEC-2025-0134](https://github.com/RustSec/advisory-db/blob/309392b18f6b17022f82c701f300433126901068/crates/rustls-pemfile/RUSTSEC-2025-0134.md) | rustls-pemfile 2.2.0 | Offiziell unmaintained; kein Patch. PemObject in rustls-pki-types ab 1.9 als Migrationsziel. | Ja |
| [RUSTSEC-2026-0440](https://github.com/RustSec/advisory-db/blob/309392b18f6b17022f82c701f300433126901068/crates/astral-tokio-tar/RUSTSEC-2026-0440.md) | astral-tokio-tar 0.6.4 | Seit 2026-11-03 offiziell unmaintained, alle Versionen; kein Patch. | **Nein** |
| [RUSTSEC-2024-0449](https://github.com/RustSec/advisory-db/blob/309392b18f6b17022f82c701f300433126901068/crates/unsafe-libyaml/RUSTSEC-2024-0449.md) | unsafe-libyaml 0.2.11 | Offiziell archiviert/unmaintained; kein Patch. | **Nein** |

Die beiden letzten Crates gehören zur Testcontainers-Kette: `testcontainers` hängt an `astral-tokio-tar`; mit Compose kommen `docker-compose-types`, `serde_yaml` und `unsafe-libyaml` hinzu. Das sind Dev-Abhängigkeiten; ihr Auftreten beweist keine Einbindung in das veröffentlichte Service-Binary. Umgekehrt sind `testcontainers` und seine aktivierten Features im Manifest nicht durch `integration-tests` optional: das Feature schützt Test-Code, nicht die Dev-Abhängigkeitsdeklaration. [Binärmanifest](https://github.com/micschr0/gluco-hub-rs/blob/428b9192f8a623356a2ad7c436f9e3d4dbfa15b1/gluco-hub/Cargo.toml), [Lockfile](https://github.com/micschr0/gluco-hub-rs/blob/428b9192f8a623356a2ad7c436f9e3d4dbfa15b1/Cargo.lock).

Die aktuelle Rustls-0.23.46-Linie und WebPKI 0.103.16 liegen bereits in den gepatchten Bereichen der jüngsten Oktober-Einträge; das beseitigt die parallel gesperrte 0.102.8-Linie nicht. Es gibt bei diesem manuellen Versionsabgleich keine weiteren Treffer, aber ohne erfolgreichen Scannerlauf keine vollständige Sicherheits- oder Policy-Freigabe. [RustSec Rustls 2026-0349](https://github.com/RustSec/advisory-db/blob/309392b18f6b17022f82c701f300433126901068/crates/rustls/RUSTSEC-2026-0349.md), [WebPKI 2026-0382](https://github.com/RustSec/advisory-db/blob/309392b18f6b17022f82c701f300433126901068/crates/rustls-webpki/RUSTSEC-2026-0382.md).

## Features, rustls, Lizenz- und MSRV-Risiken

- HTTP: `reqwest` hat `default-features = false` und `json`, `rustls`, `gzip`; MQTT verwendet bei deaktivierten Defaults `use-rustls`. Im Lockfile stehen keine Pakete `openssl`, `openssl-sys` oder `native-tls`; `openssl-probe` dient der Zertifikatssuche und ist kein Nachweis für OpenSSL-TLS. Das entspricht der deklarierten rustls-Vorgabe; ein neu aufgelöster Graph muss erneut geprüft werden. [Workspace-Manifest](https://github.com/micschr0/gluco-hub-rs/blob/428b9192f8a623356a2ad7c436f9e3d4dbfa15b1/Cargo.toml), [Lockfile](https://github.com/micschr0/gluco-hub-rs/blob/428b9192f8a623356a2ad7c436f9e3d4dbfa15b1/Cargo.lock), [openssl-probe](https://crates.io/crates/openssl-probe).
- Beibehaltene Funktionsschalter: `serde/derive`, `chrono/serde`, `tokio/full`, `tower-http/{trace,cors}`, `tracing-subscriber/{json,env-filter}`, `config` nur mit TOML und ohne Defaults, `secrecy/serde`, `validator/derive`, `clap/derive`, `tokio-util/{rt,codec}`, Testcontainers/Compose, Module/Mongo und UUID/v4. Eine Reduktion ist nur bei gemessenem Build-/Größenvorteil ein eigener Vorschlag. [Beide Cargo-Manifeste](https://github.com/micschr0/gluco-hub-rs/tree/428b9192f8a623356a2ad7c436f9e3d4dbfa15b1).
- Die pemfile-Waiver-Begründung „nur native CA-Eingaben“ ist sachlich unzutreffend: der MQTT-mTLS-Pfad liest `client_cert_file` und `client_key_file` und verarbeitet sie mit `rustls_pemfile::certs/read_one`. Für die direkte Nutzung ist eine Umstellung auf `rustls-pki-types::pem::PemObject` begründet; sie entfernt jedoch nicht die transitive Nutzung durch rumqttc. [MQTT-Code](https://github.com/micschr0/gluco-hub-rs/blob/428b9192f8a623356a2ad7c436f9e3d4dbfa15b1/gluco-hub/src/sinks/mqtt/sink.rs#L284), [Waiver](https://github.com/micschr0/gluco-hub-rs/blob/428b9192f8a623356a2ad7c436f9e3d4dbfa15b1/deny.toml), [offizielle Migrationsbeschreibung](https://rustsec.org/advisories/RUSTSEC-2025-0134.html).
- DNS-Name und operator-konfigurierter Broker allein sind kein Beweis, dass CA-seitige Namensbedingungen oder distrusted Zwischenzertifikate ausgeschlossen sind. Bevor Ausnahmen erneuert werden: konkrete aktive WebPKI-Aufrufe und den konfigurierten Rustls-Verifier mit/ohne mTLS prüfen. CRL-Nutzung ebenfalls am effektiven Verifier belegen. Dieser Bericht behauptet weder erfolgreiche Ausnutzung noch erwiesene Nichterreichbarkeit. [Namensbedingungen-Advisory](https://github.com/RustSec/advisory-db/blob/309392b18f6b17022f82c701f300433126901068/crates/rustls-webpki/RUSTSEC-2026-0099.md), [distrusted-Intermediates-Advisory](https://github.com/RustSec/advisory-db/blob/309392b18f6b17022f82c701f300433126901068/crates/rustls-webpki/RUSTSEC-2026-0382.md).
- MSRV der Workspace-Basis ist 1.95, der lokale Toolchain-Pin 1.98. Keines der angegebenen direkten Updateziele fordert laut Metadaten mehr als 1.95; fehlende Angaben und transitive Auflösungen verhindern daraus eine Build-Zusage. Metadaten sämtlicher gesperrter Pakete ohne höhere deklarierte Grenze sind hilfreich, ersetzen aber `cargo +1.95 check` nicht. [Workspace](https://github.com/micschr0/gluco-hub-rs/blob/428b9192f8a623356a2ad7c436f9e3d4dbfa15b1/Cargo.toml), [Toolchain](https://github.com/micschr0/gluco-hub-rs/blob/428b9192f8a623356a2ad7c436f9e3d4dbfa15b1/rust-toolchain.toml).
- Kein Nachweis für Lizenzinkompatibilität bei direkten Updates; für den kompletten Graphen bleibt `cargo deny check` einschließlich Lizenzscanner erforderlich. Das Projekt erlaubt u. a. AGPL-3.0-or-later, MIT, Apache-2.0, BSD, ISC und CDLA-Permissive-2.0. Mehrfachversionen sind laut Policy Warnungen; z. B. die parallelen WebPKI-, Base64-, Tower-HTTP-, Syn- und Rand-Linien sind allein kein Grund für einen Bibliothekswechsel. [Policy](https://github.com/micschr0/gluco-hub-rs/blob/428b9192f8a623356a2ad7c436f9e3d4dbfa15b1/deny.toml).
- Das Dashboard bezeichnet chrono-tz, secrecy, subtle und wiremock aufgrund von Veröffentlichungsinaktivität als möglicherweise aufgegeben. Daraus folgt kein belegter Wartungsstopp. Für rustls-pemfile existiert dagegen ein ausdrücklicher RustSec-/Archivierungsnachweis. Keine weiteren Ersetzungen allein auf Inaktivitätsbasis planen. [Dashboard mit eigener Begrenzung](https://github.com/micschr0/gluco-hub-rs/issues/67), [RustSec pemfile](https://rustsec.org/advisories/RUSTSEC-2025-0134.html).

## Empfohlene Reihenfolge für eine spätere Umsetzung

1. MQTT-TLS-Waiver korrigieren und die konkreten Pfade gegen alle fünf WebPKI-Advisories bewerten; 2026-0382 ausdrücklich einbeziehen. Falls ein betroffener aktiver Pfad bleibt, einen gepflegten gepatchten Graphen herbeiführen. Kein pauschales Ignorieren der neuen Einträge.
2. Direkte PEM-Nutzung auf die gepflegte PKI-API migrieren; Transitivpfad separat nachverfolgen. Den kleinen direkten `uuid`-Patch über Lockfile-Wartung erledigen.
3. Bestehende Testcontainers-PR samt modules-Kompatibilität und Tar-/YAML-Wartungsstatus neu bewerten. Abhängigkeiten reduzieren oder ersetzen nur, wenn Pflege-, Sicherheits- oder messbarer Build-Nutzen den Umbau rechtfertigt.
4. Den dann neu aufgelösten Graphen mit Clippy, den tatsächlich betroffenen Features/Fehlerpfaden, MSRV und `cargo deny check` bestätigen; keine automatische Übernahme dieser Recherche als Upgrade-Nachweis.

## Verifikation und verbleibende Grenzen

`cargo clippy --all-targets -- -D warnings` hat an der unveränderten Basis mit **Exit 0** bestanden (`Finished dev profile … in 3m 16s`). Das Environment setzte `RUSTUP_TOOLCHAIN=1.99.1`; Cargo/Rustc meldeten 1.99.1. Damit ist dies **kein Nachweis für den Repository-Pin 1.98 oder die MSRV 1.95**, keine All-Features-Prüfung und keine Upgrade-Validierung. Ein explizites `cargo +1.98 --version` stellte per rustup automatisch 1.98.1 bereit; ein zusätzlicher Build wurde nicht ausgeführt. Manifeste und Lockfile sind unverändert.

`cargo deny --version` und `cargo audit --version` melden jeweils „no such command“; beide Scanner sind nicht installiert. Es wurden keine Audit-Werkzeuge installiert. Deshalb keine erfolgreiche Advisory-, Lizenz-, Bans- oder Sources-Prüfung behauptet. Docker-/Integrationstests und zusätzliche Tests wurden für die reine Dokumentationsrecherche nicht ausgeführt.

Rohdaten und vollständiges Clippy-Log liegen lokal, ignoriert, unter `.claude/research-logs/` im Forschungs-Worktree. Die unabhängige Wiederholung erfolgt über die verlinkten offiziellen APIs, die Basis-Manifeste und den fixierten RustSec-Snapshot. Vertrauen: hoch für direkte Versions-/Feature-Deklarationen, Abhängigkeitskanten und Advisory-Versionsbereiche; begrenzt für aktive Erreichbarkeit, vollständige transitive Aktualität, tatsächliche MSRV-Kompatibilität und Lizenzfreigabe.
