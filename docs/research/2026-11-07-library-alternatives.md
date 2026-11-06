# Library-Alternativen mit belegbarem Mehrwert

Stand und Quellenabruf: **2026-11-07 (UTC)**. Untersuchte Projektbasis: [`428b9192f8a623356a2ad7c436f9e3d4dbfa15b1`](https://github.com/micschr0/gluco-hub-rs/tree/428b9192f8a623356a2ad7c436f9e3d4dbfa15b1), identischer Ausgangsstand von `main` und `staging`. Kontext: Entscheidungsticket [Library-Alternativen mit belegbarem Mehrwert bewerten](https://github.com/micschr0/gluco-hub-rs/issues/121).

## Ergebnis und Entscheidungsmaßstab

**Empfehlung:** Den bestehenden Stack grundsätzlich beibehalten. Für den eigenen MQTT-mTLS-Code ist die Ablösung von `rustls-pemfile` durch die offiziellen PEM-APIs von `rustls-pki-types` begründet. Zwei weitere Bereiche verdienen eine abgegrenzte Anschlussentscheidung: die Aktualität der Zeitzonendaten und die alten TLS-Abhängigkeiten von `rumqttc`. Ein Komplettwechsel von Zeitmodell oder MQTT-Architektur ist durch die vorliegenden Belege nicht gerechtfertigt.

Dies ist eine Planungsunterlage: keine Änderungen an Code, Manifesten, Lockfile oder Sicherheitsausnahmen; keine Freigabe einer Migration. „Beibehalten“ bedeutet, dass für die aktuelle Aufgabe kein nachgewiesener Vorteil eines Ersatzes vorliegt. Es ist keine Garantie künftiger Wartung oder Sicherheitsfreiheit. Versionsabgleich und vollständige Advisory-Prüfung gehören zur gesonderten Versionsrecherche.

Bewertet wurden benötigte Funktionen, offizielle Wartungshinweise, Kompatibilität mit rustls/MQTT v5/IANA-Zonen/Secret-Typen und erkennbare Migrationskosten. Aufwandseinstufungen sind qualitative Ableitungen aus dem Projektcode, keine erprobten Schätzungen. Releasealter, Downloads und Popularität allein entscheiden keinen Austausch.

| Baustein | Empfehlung | Belegbarer Nutzen / Grenze | Aufwand und Entscheidungsvorbehalt |
|---|---|---|---|
| `rustls-pemfile` direkt | **Ersetzen** durch `rustls-pki-types::pem::PemObject` | Offizieller Nachfolger, archiviertes Altprojekt; eigener Wrapper entfällt. [Upstream](https://github.com/rustls/pemfile) | Klein, im `sink-mqtt`-Feature. Entfernt die transitive Abhängigkeit von `rumqttc` noch nicht. |
| `chrono` | **Beibehalten / regulär aktualisieren** | Domaintypen und Serde-Verträge sind breit eingebunden; keine belegte Notwendigkeit eines Austauschs. [Projektmodell][P-model] | Ein Gesamtwechsel hätte hohe Folgekosten in API, DLQ, Polling und Tests. |
| `chrono-tz` | **Weiter prüfen**, gezielten Ersatz bevorzugt untersuchen | Veraltete Daten plus ausdrücklicher Wartungsvorbehalt des Maintainers. [Wartungsaussage](https://github.com/chronotope/chrono-tz/issues/232) | Jiff nur an der LLU-Grenze oder systembasierte TZif-Anbindung vergleichen; Chrono-Domaintypen können bleiben. |
| `secrecy` | **Beibehalten / regulär aktualisieren** | Explizites Offenlegen, redigiertes Debug, Löschen bei Drop passen zum Projekt. [Dokumentation](https://docs.rs/secrecy/0.10.3/secrecy/) | `zeroize` allein ersetzt den Schutzvertrag nicht; stärkere Speicherschutzmechanismen sind eine andere Anforderung. |
| `subtle` | **Beibehalten** | Bereits auf den benötigten zeitunabhängigen Bytevergleich begrenzt. [Projekt-Auth][P-auth] | Ersatz bietet derzeit keinen belegten Sicherheitsgewinn. |
| `wiremock` | **Beibehalten** | Deckt lokale asynchrone HTTP-Tests ab. [Dokumentation](https://docs.rs/wiremock/0.6.5/wiremock/) | `httpmock`/`mockito` erst bei konkreter Funktionslücke oder Wartungsblockade. |
| `rumqttc` MQTT v5 | **Weiter prüfen**, Wartungsbehebung vor Clientwechsel | Alte TLS-Abhängigkeiten sind konkret; ein genereller Laufzeitfehler im Projekt ist daraus nicht bewiesen. [Manifest](https://github.com/bytebeamio/rumqtt/blob/rumqttc-0.25.1/rumqttc/Cargo.toml) | Upstream-Lösung und `rumqttc-v5-next` vergleichen; Fork ist kein Drop-in-Ersatz. |
| `async-trait` | **Beibehalten / regulär aktualisieren** | `Arc<dyn Source>` und `Arc<dyn Sink>` brauchen Typlöschung. [Rust-Referenz](https://doc.rust-lang.org/reference/items/traits.html#dyn-compatibility) | Native `async fn` in Traits und `trait-variant` lösen diesen Fall nicht direkt. |
| `config` + `validator` | **Beibehalten / regulär aktualisieren** | TOML/ENV-Schichtung und verschachtelte/custom Validierung entsprechen dem Bedarf. [Projektkonfiguration][P-config] | Figment/Garde wären Vertragsmigrationen ohne nachgewiesenen aktuellen Zusatznutzen. |
| axum / reqwest / Tokio | **Beibehalten** | In dieser Untersuchung kein funktionaler Ersatzgrund festgestellt. [Workspace][P-cargo] | Kein Performancevergleich oder Framework-Audit durchgeführt; daraus folgt keine Überlegenheitsbehauptung. |

## Was die Inaktivitätsmeldungen tatsächlich belegen

Die fünf Hinweise aus dem Dependency Dashboard müssen unterschiedlich behandelt werden:

| Bibliothek | Verifizierter Befund beim Abruf | Einordnung |
|---|---|---|
| `rustls-pemfile` | Repository seit **2025-08-19 archiviert**; README beschreibt die Übernahme der Funktion in `rustls-pki-types`. [Repository](https://github.com/rustls/pemfile), [Migration](https://github.com/rustls/pemfile/blob/1e4ce69dd97b6bd0657a9dee8503cb8657b49235/README.md) | Tatsächlicher offizieller Ablösungspfad. |
| `chrono-tz` | Nicht archiviert; jüngster Main-Commit **2025-10-10**; Maintainer kommentiert am **2026-03-03**, er plane noch einige TZ-Updates, wolle aber auch diese schließlich einstellen. Wörtlich: “I do eventually want to stop doing that, too.” [Metadaten](https://api.github.com/repos/chronotope/chrono-tz), [Commit](https://github.com/chronotope/chrono-tz/commit/64c3e4daa9fa5a4ee4d5a6098609790142552fe5), [Aussage](https://github.com/chronotope/chrono-tz/issues/232) | Reales Wartungsrisiko; keine bestätigte sofortige Einstellung aller Updates. |
| `secrecy` | Nicht archivierter Workspace; letzter Commit unter `secrecy/` **2026-09-02**. [Metadaten](https://api.github.com/repos/iqlusioninc/crates), [Commit](https://github.com/iqlusioninc/crates/commit/d4994d154abc40b90df5d1bde29ad58280e886f3) | Releaseinaktivität allein wäre hier besonders irreführend. Workspace-Aktivität ist keine Wartungszusage für jede Crate. |
| `subtle` | Nicht archiviert; letzter Main-Commit **2024-06-19**. [Metadaten](https://api.github.com/repos/dalek-cryptography/subtle), [Commit](https://github.com/dalek-cryptography/subtle/commit/369e7463e85921377a5f2df80aabcbbc6d57a930) | Geringe Aktivität belegt; Aufgabeerklärung in den geprüften Quellen nicht gefunden. |
| `wiremock` | Nicht archiviert; letzter Main-Commit **2025-08-24**. [Metadaten](https://api.github.com/repos/LukeMathWalker/wiremock-rs), [Commit](https://github.com/LukeMathWalker/wiremock-rs/commit/6b193047bf2c5626da5dc5f3a23b58ab9bd3f130) | Wartung beobachten; kein belegter Zwang, funktionierende Tests umzuschreiben. |

Metadaten wurden über die offizielle GitHub-API abgerufen; „letzter Commit“ meint den Default-Branch, nicht alle Branches, Issues oder unveröffentlichte Arbeiten. Negative Befunde betreffen nur die geprüften Quellen, keine vollständige Durchsicht sämtlicher Diskussionen.

## PEM: gezielt auf den offiziellen Nachfolger wechseln

Im Projekt ist `rustls-pemfile` optional über `sink-mqtt` eingebunden. Der eigene Einsatz beschränkt sich auf Zertifikatskette und privaten Schlüssel im MQTT-mTLS-Aufbau. Unterstützt werden SEC1, PKCS#1 und PKCS#8; bei fehlendem Schlüssel entsteht ein `MqttError::ClientCert`. [Feature und Dependency][P-bin-cargo], [Implementierung][P-mqtt]

Upstream dokumentiert `CertificateDer::pem_reader_iter` als Ersatz für `certs` sowie `PrivateKeyDer::from_pem_reader` für das Laden eines Schlüssels. `PemObject` bietet auch Slice-APIs und meldet bei einem fehlenden passenden Einzelobjekt `NoItemsFound`; `PrivateKeyDer` unterstützt alle drei derzeit verwendeten Schlüsselvarianten. [Migrationshinweise](https://github.com/rustls/pemfile/blob/1e4ce69dd97b6bd0657a9dee8503cb8657b49235/README.md), [PemObject](https://docs.rs/rustls-pki-types/1.15.1/rustls_pki_types/pem/trait.PemObject.html), [Schlüsseltypen](https://docs.rs/rustls-pki-types/1.15.1/rustls_pki_types/enum.PrivateKeyDer.html)

**Nutzen:** direkter Bezug der gepflegten API und weniger eigener Auswahlcode. **Kein zusätzlicher Sicherheitsgewinn behauptet:** `rustls-pemfile` 2.2.0 delegiert bereits an diese Implementierung; das ist in erster Linie eine Wartungsbereinigung. [Upstream-README](https://github.com/rustls/pemfile/blob/1e4ce69dd97b6bd0657a9dee8503cb8657b49235/README.md)

Vor einer Umsetzung sind Feature-Gating, Typkompatibilität mit dem von `rumqttc` exportierten rustls sowie Verhalten bei leerer Kette, leerer/ungültiger Schlüsseldatei, vorangestellten fremden PEM-Blöcken und allen drei Schlüsselformaten zu prüfen. Bestehende Fehlerklassifikation soll erhalten bleiben. Das Entfernen unserer direkten Dependency beseitigt `rustls-pemfile` **nicht vollständig**: `rumqttc` 0.25.1 aktiviert es weiterhin über `use-rustls-no-provider`. [Upstream-Manifest](https://github.com/bytebeamio/rumqtt/blob/rumqttc-0.25.1/rumqttc/Cargo.toml)

## Zeitverarbeitung: Datenversorgung verbessern, Domaintypen bewahren

Das Projekt verwendet `chrono::DateTime<Utc>` im gemeinsamen `Reading`, in HTTP-Antworten und Betriebszustand. `chrono-tz::Tz` wird dagegen vor allem für die konfigurierte LLU-Zeitzone verwendet. Die vorhandene Konvertierung interpretiert lokale Zeit ausdrücklich in dieser Zone, wählt bei einer doppelten Herbststunde den früheren Zeitpunkt und verwirft Zeiten in der Frühlingslücke. Diese Regeln sind bestehendes Projektverhalten, keine neue Aussage über die externe LLU-API. [Modell][P-model], [Mapping][P-mapping], [Konfiguration][P-config]

Der veröffentlichte `chrono-tz`-Stand 0.10.4 enthält **IANA 2025b**; geprüft wurde die Konstante `IANA_TZDB_VERSION` im veröffentlichten Crate-Archiv. IANA nennt beim Abruf **2026f vom 2026-10-27**, unter anderem eine Änderung für Kuba ab 2026-11-01. Das ist ein konkreter Datenrückstand. Eine fehlerhafte Konvertierung für eine tatsächlich konfigurierte Projektinstanz ist damit noch nicht nachgewiesen. [Veröffentlichtes Paket](https://crates.io/api/v1/crates/chrono-tz/0.10.4/download), [IANA](https://www.iana.org/time-zones)

| Option | Funktion und belegbarer Mehrwert | Kosten / Grenze | Empfehlung |
|---|---|---|---|
| Chrono + aktualisiertes `chrono-tz` | Behält Typen und vorhandene Semantik; neue veröffentlichte TZDB-Daten könnten das Problem mit geringstem Aufwand lösen. [Chrono](https://github.com/chronotope/chrono/blob/6adaa5240c26fecb7bd9077334a91f8f67f4f3fe/README.md) | Zum Abruf kein neuerer veröffentlichter TZDB-Stand belegt; Wartungsvorbehalt bleibt. | Aktualisierung vor Austausch prüfen, aber nicht unbegrenzt darauf vertrauen. |
| Jiff nur in LLU-Zonenauflösung/Mapping; Ausgabe weiter Chrono-UTC | IANA-Zonen, System-TZDB und optional eingebettete Daten; explizite APIs für mehrdeutige Zeiten. [Jiff](https://docs.rs/jiff/0.2.39/jiff/) | Zweite Zeitbibliothek und bewusste UTC-Konvertierung. Datenquelle muss im Container festgelegt werden; standardmäßiges Gap-Verhalten ist nicht identisch. | **Bevorzugter Kandidat zur weiteren Prüfung**, wenn aktuelle Zonen ohne `chrono-tz`-Release benötigt werden. |
| `tzfile` als Chrono-`TimeZone` | Liest System-TZif-Dateien und behält Chrono-Integration. [Dokumentation](https://docs.rs/tzfile/0.1.3/tzfile/) | Verfügbarkeit und Pflege von `/usr/share/zoneinfo`, eigene Ladefehler und Wartung der zusätzlichen Bibliothek prüfen. Der Projekt-Dockerfile allein beweist den TZDB-Inhalt des Basisimages nicht. [Dockerfile][P-docker] | Enger Gegenkandidat; weniger Typwechsel, mehr Betriebsabhängigkeit. |
| Vollständiger Wechsel auf Jiff oder `time` | Jiff hat ein vollständiges Zonenmodell; `time` dokumentiert unter anderem UTC-Offsets, Parsing und Serde. [Jiff](https://docs.rs/jiff/0.2.39/jiff/), [time](https://docs.rs/time/0.3.55/time/) | Domaintypen, Persistenz und Serialisierung betroffen. `time` allein bietet keinen belegten Ersatz für benannte IANA-Zonen. | Gegenwärtig nicht empfohlen. |

Wichtige Migrationsfalle: Jiffs Default `Compatible` wählt bei Herbstüberlappungen den früheren Zeitpunkt, verschiebt aber Zeiten in der Frühlingslücke nach vorne. Das Projekt verwirft diese Lücke. Ein einfacher Austausch der Standardmethode wäre daher nicht verhaltensgleich. [Disambiguation](https://docs.rs/jiff/0.2.39/jiff/tz/enum.Disambiguation.html), [bestehendes Mapping][P-mapping]

Die Anschlussentscheidung sollte nur die Datenversorgung und diese LLU-Grenze klären: eingebettete oder System-TZDB, Updateverantwortung, identische Gap/Fold-Regeln und UTC-Ausgabe. Weder ein neues allgemeines Zeitframework noch ein Austausch aller Chrono-Typen ist dafür erforderlich.

## Secrets und Tokenvergleich: Schutzverträge erhalten

`SecretString`/`ExposeSecret` werden in Konfiguration, LLU, Nightscout und MQTT sowie HTTP-Auth verwendet. `secrecy` dokumentiert expliziten Zugriff, Schutz vor versehentlichem Debug-Logging, Zeroisierung bei Drop und standardmäßig eingeschränkte Serialisierung. Das entspricht dem vorhandenen Bedarf. [Projektkonfiguration][P-config], [secrecy](https://docs.rs/secrecy/0.10.3/secrecy/)

`zeroize::Zeroizing<String>` wäre eine schmalere Alternative für das Löschen bei Drop, ersetzt aber nicht automatisch den gesamten Secret-Vertrag. `secrets` bietet geschützte Speicherbereiche und verändert unter UNIX im Release-Modus die Core-Dump-Einstellung. Solche Mechanismen wären bei einer ausdrücklich verlangten Speicherschutzanforderung zu prüfen; diese Recherche hat keinen entsprechenden Bedarf des Projekts nachgewiesen. Weder Wrapper verhindert alle zuvor erzeugten Kopien von Zugangsdaten. [zeroize](https://docs.rs/zeroize/1.9.1/zeroize/), [secrets](https://docs.rs/secrets/1.3.0/secrets/), [secrecy-Grenzen](https://docs.rs/secrecy/0.10.3/secrecy/)

Für Bearer-Tokens nutzt das Projekt einen expliziten Längencheck und `subtle::ConstantTimeEq`. `subtle` beschreibt seine Seitkanalresistenz als bestmöglichen Ansatz, dessen Eigenschaften auch von Compiler, Hardware und Build-Modus abhängen. `constant_time_eq` könnte die einzelne Vergleichsoperation übernehmen, lässt ebenfalls Längenabhängigkeit zu und liefert keinen hier nachgewiesenen zusätzlichen Schutz. Die bewusste Gleichlängenbedingung muss bei jedem Ersatz erhalten bleiben. **Beide bestehenden Bibliotheken beibehalten.** [Auth][P-auth], [subtle](https://docs.rs/subtle/2.6.1/subtle/), [constant_time_eq](https://docs.rs/constant_time_eq/0.6.1/constant_time_eq/)

## HTTP-Testinfrastruktur: Wechsel nur für eine konkrete fehlende Fähigkeit

`wiremock` steht ausschließlich in den Entwicklungsabhängigkeiten und wird für LLU-Authentifizierung, Quellen, Nightscout und End-to-End-HTTP-Szenarien verwendet. Es bietet asynchrone lokale Server, Matcher, dynamische Antworten und Aufrufzahl-Erwartungen; Tokio wird unterstützt. [Manifest][P-bin-cargo], [E2E-Tests][P-e2e], [Dokumentation](https://docs.rs/wiremock/0.6.5/wiremock/)

`httpmock` wäre bei benötigtem HTTPS-Mocking, Aufnahme/Wiedergabe oder Proxy-/Standalone-Betrieb interessant. Aktuelles `mockito` bietet ebenfalls parallele Tests, mehrere Hosts und asynchrone Schnittstellen. Seine heutige Dokumentation widerspricht damit der alten Vergleichstabelle in den `wiremock`-Docs; diese Tabelle ist keine verlässliche Entscheidungsbasis. [httpmock](https://docs.rs/httpmock/0.9.0/httpmock/), [mockito](https://docs.rs/mockito/1.7.2/mockito/), [alte Tabelle](https://docs.rs/wiremock/0.6.5/wiremock/#prior-art)

Keine dieser Zusatzfähigkeiten wurde als derzeitige Projektlücke nachgewiesen. Ein Wechsel würde viele Fixtures, Matcher und Erwartungssemantiken betreffen und müsste bestehende Fehlerpfade erhalten. Empfehlung: `wiremock` behalten; eine später reproduzierbare Inkompatibilität oder konkret benötigte Testfähigkeit wäre der Auslöser für einen kleinen Vergleich, nicht das Releasealter.

## MQTT: erst die enge Wartungsfrage entscheiden

Der bestehende Sink benötigt MQTT **v5**, QoS 1, Retain/LWT, Session-Expiry, Publish-Properties, Wiederverbindung und rustls einschließlich optionalem mTLS. Er verarbeitet `ConnAck`/`PubAck`, eigene Fehlerklassen und einen Test-Broker auf Grundlage des bestehenden v5-Codecs. Ein Clientwechsel berührt daher mehr als Imports. [Sink und Tests][P-mqtt], [Fehlerklassen][P-mqtt-error]

`rumqttc` bietet einen Tokio-basierten Eventloop und MQTT-v5-API. Die offizielle Roadmap nennt begrenzte Teamkapazität; daraus folgt kein Nachweis vollständiger Aufgabe. [README](https://github.com/bytebeamio/rumqtt/blob/rumqttc-0.25.1/rumqttc/README.md), [v5-Dokumentation](https://docs.rs/rumqttc/0.25.1/rumqttc/v5/), [Maintainer-Roadmap](https://github.com/bytebeamio/rumqtt/issues/942)

**Konkreter Handlungsgrund:** 0.25.1 hängt direkt von `rustls-webpki` 0.102.8 und `rustls-pemfile` 2.2.0 ab. Upstream verfolgt die alten WebPKI-Advisories in einer offenen Meldung. Das inzwischen veröffentlichte Advisory RUSTSEC-2026-0382 nennt als korrigierte stabile Reihe mindestens 0.103.16. Ob und wie der betroffene Code im konkreten Programm aufgerufen wird, ist getrennt von der Lockfile-Betroffenheit zu prüfen. [Manifest](https://github.com/bytebeamio/rumqtt/blob/rumqttc-0.25.1/rumqttc/Cargo.toml), [Upstream-Meldung](https://github.com/bytebeamio/rumqtt/issues/1046), [RustSec](https://rustsec.org/advisories/RUSTSEC-2026-0382.html)

Die gezielte Quellenprüfung begrenzt die Aussage: Im vollständigen `rumqttc/src` des offiziellen Tags `rumqttc-0.25.1` erscheint die direkte Referenz auf `webpki` nur als `Error::WebPki(#[from] webpki::Error)`. Der TLS-Aufbau delegiert an `tokio_rustls::TlsConnector` und `rustls::ClientConfig`; unser mTLS-Zweig liefert eine `TlsConfiguration::Rustls`. Das spricht für eine möglicherweise entbehrliche direkte Alt-Abhängigkeit, **beweist aber keine allgemeine Unausnutzbarkeit**. Die aktive rustls-Abhängigkeit und alle Featurekombinationen müssen in der Sicherheitsprüfung getrennt beurteilt werden. [TLS-Quellcode des Tags](https://github.com/bytebeamio/rumqtt/blob/rumqttc-0.25.1/rumqttc/src/tls.rs), [Projekt-TLS][P-mqtt]

| Weg | Eignung und Nutzen | Aufwand / verbleibendes Risiko | Empfehlung |
|---|---|---|---|
| Veröffentlichte Upstream-Wartungslösung | Erhält Eventloop, v5-Codecs und Integrationsmodell; könnte die alten direkten TLS-Dependencies bereinigen. | Zum Abruf keine passende neuere veröffentlichte Lösung belegt. Entfernung einer öffentlichen Fehlervariante kann API-Folgen haben. [Releaseübersicht](https://github.com/bytebeamio/rumqtt/releases), [TLS-Code](https://github.com/bytebeamio/rumqtt/blob/rumqttc-0.25.1/rumqttc/src/tls.rs) | Engste bevorzugte Lösung, falls rechtzeitig verfügbar; keine neue dauerhafte Advisory-Ausnahme daraus ableiten. |
| `rumqttc-v5-next` / `rumqttc-next` | Fork mit explizitem MQTT-v5-Paket, rustls und neueren TLS-Dependencies; direkte Nachfolgefamilie. [Paket 0.34.0](https://docs.rs/crate/rumqttc-v5-next/0.34.0), [Facade](https://docs.rs/crate/rumqttc-next/0.34.0) | Echte API-Migration: flache v5-Pfade, Builder, `PublishOptions`, Auth-/Transportdetails. Höhere Aktivität ist keine Sicherheitsgarantie; Maintainer-/Releasevertrauen zusätzlich prüfen. [Migrationsleitfaden](https://github.com/thehouseisonfire/rumqtt/blob/main/MIGRATION.md) | **Konkreter Ersatzkandidat zur weiteren Prüfung**, wenn die Upstream-Lösung ausbleibt. |
| Eclipse Paho Rust | MQTT v5 und etablierte C-Bibliothek. [Upstream](https://github.com/eclipse-paho/paho.mqtt.rust) | FFI/C-Build; TLS hängt laut offizieller Anleitung von OpenSSL ab. Das kollidiert mit der rustls-/No-OpenSSL-Vorgabe. | Für dieses Projekt nicht empfohlen. |

Für den Fork nennt der veröffentlichte Paketstand 0.34.0 `rustls-webpki ^0.103.12`; diese Range erlaubt 0.103.16, garantiert aber ohne neue Auflösung und Prüfung keinen sicheren Lockfile-Stand. Der dynamische Main-Migrationsleitfaden enthält zudem Beispiele für `0.34.0-alpha` und Änderungen gegenüber veröffentlichten Releases. Er belegt Migrationskosten, ersetzt aber keine Prüfung des tatsächlich ausgewählten stabilen Pakets. [Paketabhängigkeiten](https://docs.rs/crate/rumqttc-v5-next/0.34.0), [Leitfaden](https://github.com/thehouseisonfire/rumqtt/blob/main/MIGRATION.md)

Eine spätere begrenzte Evaluation sollte Publish/Properties/QoS-1-Acknowledgements, LWT/Retain, Session-Expiry, Wiederverbindung, Shutdown, mTLS und Fehlerklassifikation gegen die vorhandenen Offline- und Mosquitto-Szenarien prüfen. Das ist eine vorgeschlagene Verifikation, kein in dieser Recherche durchgeführter Test. MQTT v3 allein oder ein Umbau des Source/Sink-Modells wären keine gleichwertigen Ersatzlösungen.

## Async-Traits und Konfiguration: aktuelle Anforderungen sprechen für Bestand

Die Kerntraits sind `Send + Sync` und werden im Poller, Router und DLQ als `Arc<dyn Source>` beziehungsweise `Arc<dyn Sink>` verwendet. Native asynchrone Traitmethoden sind laut aktueller Rust-Referenz nicht dyn-kompatibel. `async-trait` erzeugt die dafür notwendige Typlöschung. `trait-variant` generiert unter anderem `Send`-Varianten mit `impl Future`; das ersetzt diese dynamische Dispatch-Grenze nicht. [Source][P-source], [Sink][P-sink], [Wiring][P-main], [Rust-Referenz](https://doc.rust-lang.org/reference/items/traits.html#dyn-compatibility), [async-trait](https://docs.rs/async-trait/0.1.92/async_trait/), [trait-variant](https://docs.rs/trait-variant/0.1.3/trait_variant/)

Manuell zurückgegebene `Pin<Box<dyn Future + Send>>` könnten das Makro entfernen, verlagern aber den Aufwand in die öffentliche Schnittstelle. Eine Umstellung auf Generics oder einen geschlossenen Enum würde die vorhandene Erweiterungsgrenze ändern. Ohne gemessenes Problem rechtfertigt die Makroentfernung diesen Umbau nicht. Das ist eine Architekturabwägung, kein gemessener Performancebefund.

`config` schichtet im Projekt Defaults, optionale TOML-Datei und `GLUCO_HUB__…`-Umgebungsvariablen; danach folgen Serde-Deserialisierung und `validator` einschließlich verschachtelter, feldbezogener und Schema-Regeln. Die Bibliotheken dokumentieren die benötigten Schichtungs- und Validierungsfunktionen. [Projektcode][P-config], [config](https://docs.rs/config/0.15.27/config/), [validator](https://docs.rs/validator/0.21.0/validator/)

Figment bietet ebenfalls Provider-Schichtung, Profile und Herkunftsmetadaten; Garde bietet unter anderem Validierung innerer Typen, Newtypes, Kontext und eigene Regeln. Diese Funktionen sind reale Alternativen, aber ein im Projekt fehlendes Erfordernis dafür wurde nicht festgestellt. Eine Migration müsste ENV-Präfix/Trennzeichen, Typkonvertierung, Merge-Prioritäten, Fehlermeldungen, Secret-Deserialisierung und Schema-Regeln erhalten. **Beide bestehenden Bibliotheken behalten.** [Figment](https://docs.rs/figment/0.10.19/figment/), [ENV-Verhalten](https://docs.rs/figment/0.10.19/figment/providers/struct.Env.html), [Garde](https://docs.rs/garde/0.23.0/garde/)

## Grenzen und Übergabe

- **Belegt:** Projektverwendung am angegebenen SHA, offizieller PEM-Nachfolger, abgegrenzte Wartungssignale, TZDB-Rückstand, dokumentierte Alternativfunktionen und erkennbare API-Brüche.
- **Abgeleitet:** Vorrang enger Wartungsänderungen vor vollständigem Austausch; Jiff an der LLU-Grenze und `rumqttc-v5-next` sind Prüfungskandidaten. Keine dieser Migrationen wurde gebaut oder gemessen.
- **Offen:** tatsächliche Deployment-Zonen, Inhalt/Updatezyklus der Container-TZDB, ausgewählter Fork-Release und dessen Governance sowie vollständige Laufzeiterreichbarkeit der TLS-Advisories. Diese Punkte rechtfertigen Anschlussentscheidungen, keine stillschweigende Umsetzung.
- **Quellengrenze:** Abrufstand 2026-11-07. Versionierte Docs und Tags sind bevorzugt verlinkt; GitHub-Issues, Metadaten, IANA-Seite und Fork-Main-Dokumentation bleiben dynamisch. Die paketbezogenen Versionsnummern dienen der reproduzierbaren Quellenzuordnung, nicht als erschöpfende Liste aller aktuellen Versionen.
- **Verifikation:** Quellen und Baseline-Code gelesen; offizielles rumqttc-Tag-Archiv nach direkten WebPKI-Aufrufen durchsucht; veröffentlichte TZDB-Konstante gelesen. Keine Builds, Migrationstests oder Benchmarks durchgeführt. Der gemeinsame Baseline-Clippy-Lauf wird von der separaten Versionsrecherche dokumentiert. Nur dieser Bericht wird auf dem Forschungsbranch gespeichert; keine PR, kein Merge, keine Veröffentlichung einer Softwareänderung.

[P-cargo]: https://github.com/micschr0/gluco-hub-rs/blob/428b9192f8a623356a2ad7c436f9e3d4dbfa15b1/Cargo.toml
[P-bin-cargo]: https://github.com/micschr0/gluco-hub-rs/blob/428b9192f8a623356a2ad7c436f9e3d4dbfa15b1/gluco-hub/Cargo.toml
[P-model]: https://github.com/micschr0/gluco-hub-rs/blob/428b9192f8a623356a2ad7c436f9e3d4dbfa15b1/gluco-hub-core/src/model.rs
[P-auth]: https://github.com/micschr0/gluco-hub-rs/blob/428b9192f8a623356a2ad7c436f9e3d4dbfa15b1/gluco-hub/src/api/auth.rs
[P-config]: https://github.com/micschr0/gluco-hub-rs/blob/428b9192f8a623356a2ad7c436f9e3d4dbfa15b1/gluco-hub/src/config.rs
[P-mqtt]: https://github.com/micschr0/gluco-hub-rs/blob/428b9192f8a623356a2ad7c436f9e3d4dbfa15b1/gluco-hub/src/sinks/mqtt/sink.rs
[P-mqtt-error]: https://github.com/micschr0/gluco-hub-rs/blob/428b9192f8a623356a2ad7c436f9e3d4dbfa15b1/gluco-hub/src/sinks/mqtt/error.rs
[P-mapping]: https://github.com/micschr0/gluco-hub-rs/blob/428b9192f8a623356a2ad7c436f9e3d4dbfa15b1/gluco-hub/src/sources/llu/mapping.rs
[P-docker]: https://github.com/micschr0/gluco-hub-rs/blob/428b9192f8a623356a2ad7c436f9e3d4dbfa15b1/Dockerfile
[P-e2e]: https://github.com/micschr0/gluco-hub-rs/blob/428b9192f8a623356a2ad7c436f9e3d4dbfa15b1/gluco-hub/src/e2e_tests.rs
[P-source]: https://github.com/micschr0/gluco-hub-rs/blob/428b9192f8a623356a2ad7c436f9e3d4dbfa15b1/gluco-hub-core/src/source.rs
[P-sink]: https://github.com/micschr0/gluco-hub-rs/blob/428b9192f8a623356a2ad7c436f9e3d4dbfa15b1/gluco-hub-core/src/sink.rs
[P-main]: https://github.com/micschr0/gluco-hub-rs/blob/428b9192f8a623356a2ad7c436f9e3d4dbfa15b1/gluco-hub/src/main.rs
