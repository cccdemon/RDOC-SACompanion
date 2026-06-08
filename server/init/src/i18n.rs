//! Translations for the server-rendered pages (EN/DE/IT/ES/FR). Language is
//! chosen from `?lang=xx`, else the Accept-Language header, else English.

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Lang {
    En,
    De,
    It,
    Es,
    Fr,
}

pub const GITHUB_URL: &str = "https://github.com/cccdemon/RDOC-SquadLinkLite";
pub const RAUMDOCK_URL: &str = "https://raumdock.org";
pub const FLEET_URL: &str = "https://suite.raumdock.org/fleetplanner";

impl Lang {
    pub fn code(self) -> &'static str {
        match self {
            Lang::En => "en",
            Lang::De => "de",
            Lang::It => "it",
            Lang::Es => "es",
            Lang::Fr => "fr",
        }
    }
    pub fn all() -> [Lang; 5] {
        [Lang::En, Lang::De, Lang::It, Lang::Es, Lang::Fr]
    }
    fn parse(code: &str) -> Option<Lang> {
        match code.trim().to_ascii_lowercase().get(0..2)? {
            "en" => Some(Lang::En),
            "de" => Some(Lang::De),
            "it" => Some(Lang::It),
            "es" => Some(Lang::Es),
            "fr" => Some(Lang::Fr),
            _ => None,
        }
    }
    /// `?lang=` wins; else the first matching Accept-Language tag; else English.
    pub fn detect(query: Option<&str>, accept: Option<&str>) -> Lang {
        if let Some(q) = query {
            if let Some(l) = Lang::parse(q) {
                return l;
            }
        }
        if let Some(a) = accept {
            for part in a.split(',') {
                let tag = part.split(';').next().unwrap_or("");
                if let Some(l) = Lang::parse(tag) {
                    return l;
                }
            }
        }
        Lang::En
    }
}

/// Footer nav labels: [Download, Privacy, Legal, License].
pub fn nav(l: Lang) -> [&'static str; 4] {
    match l {
        Lang::En => ["Download", "Privacy", "Legal notice", "License"],
        Lang::De => ["Download", "Datenschutz", "Impressum", "Lizenz"],
        Lang::It => ["Download", "Privacy", "Note legali", "Licenza"],
        Lang::Es => ["Descargar", "Privacidad", "Aviso legal", "Licencia"],
        Lang::Fr => ["Télécharger", "Confidentialité", "Mentions légales", "Licence"],
    }
}

/// Language switcher: links to the same path with each `?lang=`.
pub fn switcher(path: &str, cur: Lang) -> String {
    let mut s = String::from("<nav class=\"lang\">");
    for l in Lang::all() {
        let on = if l == cur { " class=\"on\"" } else { "" };
        s.push_str(&format!(
            "<a href=\"{path}?lang={code}\"{on}>{label}</a>",
            code = l.code(),
            label = l.code().to_uppercase(),
        ));
    }
    s.push_str("</nav>");
    s
}

// ── Pages ────────────────────────────────────────────────────────────────────

pub fn home(l: Lang, base: &str) -> (&'static str, String) {
    let links = |raumdock: &str, fleet: &str, src: &str, legal: &str, priv_: &str, lic: &str| {
        format!(
            r#"<h2>{lh}</h2>
<p class="links">
<a href="{RAUMDOCK_URL}">{raumdock}</a>
<a href="{FLEET_URL}">{fleet}</a>
<a href="{GITHUB_URL}">{src}</a>
</p>
<p class="muted"><a href="/privacy?lang={lc}">{priv_}</a> · <a href="/legal?lang={lc}">{legal}</a> · <a href="/license?lang={lc}">{lic}</a></p>"#,
            lh = "Links",
            lc = l.code(),
        )
    };
    match l {
        Lang::En => (
            "What is this?",
            format!(
                r#"<h1>RDOC SquadLink Lite</h1>
<p>A simple peer-to-peer voice chat for small groups. Push-to-talk, no account, no recording, encrypted.</p>
<p>Voice flows directly between players (WebRTC/Opus). There is no server listening in — a small service only sets up the connection.</p>
<h2>How it works</h2>
<ul>
<li>The host creates a session in the app and gets a link and a 6-digit PIN.</li>
<li>Mates open the link, install the app, enter code and PIN.</li>
<li>The session stays alive while members are connected (max. 24&nbsp;hours).</li>
</ul>
<p><a class="dl" href="{base}/download/">Download the app (Windows)</a></p>
<p class="muted">Prototype, unsigned. Windows SmartScreen: "More info" then "Run anyway".</p>
{links}"#,
                links = links("raumdock.org", "RDOC Fleet Manager", "Source on GitHub", "Legal notice", "Privacy", "License")
            ),
        ),
        Lang::De => (
            "Was ist das?",
            format!(
                r#"<h1>RDOC SquadLink Lite</h1>
<p>Ein einfacher Peer-to-Peer-Voice-Chat für kleine Gruppen. Push-to-Talk, ohne Account, ohne Aufnahme, verschlüsselt.</p>
<p>Die Stimme läuft direkt zwischen den Spielern (WebRTC/Opus). Es gibt keinen Server, der mithört — ein kleiner Dienst stellt nur die Verbindung her.</p>
<h2>So funktioniert es</h2>
<ul>
<li>Host erstellt in der App eine Session und erhält einen Link und eine 6-stellige PIN.</li>
<li>Mitspieler öffnen den Link, installieren die App, geben Code und PIN ein.</li>
<li>Die Session bleibt bestehen, solange Teilnehmer verbunden sind (maximal 24&nbsp;Stunden).</li>
</ul>
<p><a class="dl" href="{base}/download/">App herunterladen (Windows)</a></p>
<p class="muted">Prototyp, unsigniert. Windows SmartScreen: „Weitere Informationen" und „Trotzdem ausführen".</p>
{links}"#,
                links = links("raumdock.org", "RDOC Fleetmanager", "Quellcode auf GitHub", "Impressum", "Datenschutz", "Lizenz")
            ),
        ),
        Lang::It => (
            "Cos'è?",
            format!(
                r#"<h1>RDOC SquadLink Lite</h1>
<p>Una semplice chat vocale peer-to-peer per piccoli gruppi. Push-to-talk, senza account, senza registrazione, cifrata.</p>
<p>La voce passa direttamente tra i giocatori (WebRTC/Opus). Nessun server in ascolto — un piccolo servizio stabilisce solo la connessione.</p>
<h2>Come funziona</h2>
<ul>
<li>L'host crea una sessione nell'app e ottiene un link e un PIN di 6 cifre.</li>
<li>I compagni aprono il link, installano l'app, inseriscono codice e PIN.</li>
<li>La sessione resta attiva finché ci sono partecipanti connessi (max 24&nbsp;ore).</li>
</ul>
<p><a class="dl" href="{base}/download/">Scarica l'app (Windows)</a></p>
<p class="muted">Prototipo, non firmato. Windows SmartScreen: "Ulteriori informazioni" e "Esegui comunque".</p>
{links}"#,
                links = links("raumdock.org", "RDOC Fleet Manager", "Codice su GitHub", "Note legali", "Privacy", "Licenza")
            ),
        ),
        Lang::Es => (
            "¿Qué es esto?",
            format!(
                r#"<h1>RDOC SquadLink Lite</h1>
<p>Un chat de voz peer-to-peer sencillo para grupos pequeños. Pulsar para hablar, sin cuenta, sin grabación, cifrado.</p>
<p>La voz viaja directamente entre los jugadores (WebRTC/Opus). Ningún servidor escucha — un pequeño servicio solo establece la conexión.</p>
<h2>Cómo funciona</h2>
<ul>
<li>El anfitrión crea una sesión en la app y obtiene un enlace y un PIN de 6 dígitos.</li>
<li>Los compañeros abren el enlace, instalan la app e introducen código y PIN.</li>
<li>La sesión permanece activa mientras haya participantes conectados (máx. 24&nbsp;horas).</li>
</ul>
<p><a class="dl" href="{base}/download/">Descargar la app (Windows)</a></p>
<p class="muted">Prototipo, sin firmar. Windows SmartScreen: "Más información" y "Ejecutar de todas formas".</p>
{links}"#,
                links = links("raumdock.org", "RDOC Fleet Manager", "Código en GitHub", "Aviso legal", "Privacidad", "Licencia")
            ),
        ),
        Lang::Fr => (
            "Qu'est-ce que c'est ?",
            format!(
                r#"<h1>RDOC SquadLink Lite</h1>
<p>Un chat vocal pair-à-pair simple pour petits groupes. Push-to-talk, sans compte, sans enregistrement, chiffré.</p>
<p>La voix circule directement entre les joueurs (WebRTC/Opus). Aucun serveur n'écoute — un petit service établit seulement la connexion.</p>
<h2>Comment ça marche</h2>
<ul>
<li>L'hôte crée une session dans l'app et obtient un lien et un code PIN à 6 chiffres.</li>
<li>Les coéquipiers ouvrent le lien, installent l'app, saisissent le code et le PIN.</li>
<li>La session reste active tant que des participants sont connectés (max. 24&nbsp;heures).</li>
</ul>
<p><a class="dl" href="{base}/download/">Télécharger l'app (Windows)</a></p>
<p class="muted">Prototype, non signé. Windows SmartScreen : « Informations complémentaires » puis « Exécuter quand même ».</p>
{links}"#,
                links = links("raumdock.org", "RDOC Fleet Manager", "Code sur GitHub", "Mentions légales", "Confidentialité", "Licence")
            ),
        ),
    }
}

pub fn privacy(l: Lang) -> (&'static str, String) {
    match l {
        Lang::En => ("Privacy", PRIVACY_EN.into()),
        Lang::De => ("Datenschutz", PRIVACY_DE.into()),
        Lang::It => ("Privacy", PRIVACY_IT.into()),
        Lang::Es => ("Privacidad", PRIVACY_ES.into()),
        Lang::Fr => ("Confidentialité", PRIVACY_FR.into()),
    }
}

pub fn legal(l: Lang) -> (&'static str, String) {
    match l {
        Lang::En => ("Legal notice", LEGAL_EN.into()),
        Lang::De => ("Impressum", LEGAL_DE.into()),
        Lang::It => ("Note legali", LEGAL_IT.into()),
        Lang::Es => ("Aviso legal", LEGAL_ES.into()),
        Lang::Fr => ("Mentions légales", LEGAL_FR.into()),
    }
}

pub fn license(l: Lang) -> (&'static str, String) {
    let body = |intro: &str, h: &str, b1: &str, b2: &str, b3: &str, b4: &str, ch: &str, ct: &str, mail: &str, sumh: &str, full: &str, foot: &str| {
        format!(
            r#"<h1>{intro}</h1>
<p>RDOC SquadLink Lite — <b>PolyForm Noncommercial License 1.0.0</b>.</p>
<h2>{h}</h2>
<ul><li>{b1}</li><li>{b2}</li><li>{b3}</li><li>{b4}</li></ul>
<h2>{ch}</h2>
<p>{ct}</p>
<p>{mail}: <a href="mailto:commercialusage@raumdock.org">commercialusage@raumdock.org</a></p>
<p>{sumh}</p>
<p><a class="dl" href="{GITHUB_URL}/blob/main/LICENSE">{full}</a></p>
<p class="muted">© head87x &amp; justcallmedeimos. {foot}</p>"#
        )
    };
    match l {
        Lang::En => ("License", body(
            "License — non-commercial", "In short",
            "Use, copy, modify, share — for any non-commercial purpose (private, community, education, research).",
            "No commercial use without a separate license.",
            "Keep the license and copyright notices.",
            "Provided as is, without warranty or liability.",
            "Commercial use", "Commercial use requires a separate commercial license: selling, sublicensing, hosting as a paid service, integrating into commercial products, or use in revenue-generating activities.",
            "Inquiries", "This is a summary — the full license text is binding:", "View the full license (LICENSE)", "PolyForm Noncommercial License 1.0.0 — see polyformproject.org.")),
        Lang::De => ("Lizenz", body(
            "Lizenz — nicht-kommerziell", "Kurz gesagt",
            "Nutzen, kopieren, ändern, weitergeben — für jeden nicht-kommerziellen Zweck (privat, Community, Bildung, Forschung).",
            "Keine kommerzielle Nutzung ohne gesonderte Lizenz.",
            "Lizenz- und Urhebervermerke beibehalten.",
            "Ohne Gewähr / ohne Haftung.",
            "Kommerzielle Nutzung", "Kommerzielle Nutzung erfordert eine separate kommerzielle Lizenz: Verkauf, Unterlizenzierung, Betrieb als bezahlter Dienst, Integration in kommerzielle Produkte oder Nutzung in umsatzgenerierenden Aktivitäten.",
            "Anfragen", "Dies ist eine Zusammenfassung — verbindlich ist der vollständige Lizenztext:", "Vollständige Lizenz (LICENSE) ansehen", "PolyForm Noncommercial License 1.0.0 — siehe polyformproject.org.")),
        Lang::It => ("Licenza", body(
            "Licenza — non commerciale", "In breve",
            "Usare, copiare, modificare, condividere — per qualsiasi scopo non commerciale (privato, community, istruzione, ricerca).",
            "Nessun uso commerciale senza una licenza separata.",
            "Mantenere gli avvisi di licenza e copyright.",
            "Fornito così com'è, senza garanzie né responsabilità.",
            "Uso commerciale", "L'uso commerciale richiede una licenza commerciale separata: vendita, sublicenza, hosting come servizio a pagamento, integrazione in prodotti commerciali o uso in attività che generano ricavi.",
            "Richieste", "Questo è un riassunto — fa fede il testo completo della licenza:", "Vedi la licenza completa (LICENSE)", "PolyForm Noncommercial License 1.0.0 — vedi polyformproject.org.")),
        Lang::Es => ("Licencia", body(
            "Licencia — no comercial", "En resumen",
            "Usar, copiar, modificar, compartir — para cualquier fin no comercial (privado, comunidad, educación, investigación).",
            "Sin uso comercial sin una licencia aparte.",
            "Conservar los avisos de licencia y copyright.",
            "Se ofrece tal cual, sin garantía ni responsabilidad.",
            "Uso comercial", "El uso comercial requiere una licencia comercial aparte: venta, sublicencia, alojamiento como servicio de pago, integración en productos comerciales o uso en actividades que generan ingresos.",
            "Consultas", "Esto es un resumen — el texto completo de la licencia es vinculante:", "Ver la licencia completa (LICENSE)", "PolyForm Noncommercial License 1.0.0 — ver polyformproject.org.")),
        Lang::Fr => ("Licence", body(
            "Licence — non commerciale", "En bref",
            "Utiliser, copier, modifier, partager — pour tout usage non commercial (privé, communauté, éducation, recherche).",
            "Aucun usage commercial sans licence distincte.",
            "Conserver les mentions de licence et de droit d'auteur.",
            "Fourni en l'état, sans garantie ni responsabilité.",
            "Usage commercial", "L'usage commercial nécessite une licence commerciale distincte : vente, sous-licence, hébergement en service payant, intégration dans des produits commerciaux ou usage dans des activités génératrices de revenus.",
            "Demandes", "Ceci est un résumé — le texte complet de la licence fait foi :", "Voir la licence complète (LICENSE)", "PolyForm Noncommercial License 1.0.0 — voir polyformproject.org.")),
    }
}

/// Share-link landing in the chosen language (`code` is already HTML-escaped).
pub fn landing(l: Lang, base: &str, code: &str) -> String {
    let (intro, codelbl, step1, step2, btn, foot) = match l {
        Lang::En => ("You have been invited to a voice session.", "Session code:", "App not installed? Download here", "Open the app → Join → enter the code + the 6-digit PIN (from the host).", "Download SquadLink Lite", "Audio runs directly peer-to-peer (encrypted). The server only brokers."),
        Lang::De => ("Du wurdest zu einer Voice-Session eingeladen.", "Session-Code:", "App noch nicht installiert? Hier herunterladen", "App öffnen → Beitreten → Code + die 6-stellige PIN (vom Host) eingeben.", "SquadLink Lite herunterladen", "Audio läuft direkt Peer-zu-Peer (verschlüsselt). Der Server vermittelt nur."),
        Lang::It => ("Sei stato invitato a una sessione vocale.", "Codice sessione:", "App non installata? Scaricala qui", "Apri l'app → Partecipa → inserisci il codice + il PIN di 6 cifre (dall'host).", "Scarica SquadLink Lite", "L'audio è diretto peer-to-peer (cifrato). Il server fa solo da tramite."),
        Lang::Es => ("Te han invitado a una sesión de voz.", "Código de sesión:", "¿App no instalada? Descárgala aquí", "Abre la app → Unirse → introduce el código + el PIN de 6 dígitos (del anfitrión).", "Descargar SquadLink Lite", "El audio es directo peer-to-peer (cifrado). El servidor solo intermedia."),
        Lang::Fr => ("Vous avez été invité à une session vocale.", "Code de session :", "App non installée ? Téléchargez-la ici", "Ouvrez l'app → Rejoindre → saisissez le code + le PIN à 6 chiffres (de l'hôte).", "Télécharger SquadLink Lite", "L'audio est direct pair-à-pair (chiffré). Le serveur ne fait que l'intermédiaire."),
    };
    format!(
        r#"<h1>RDOC SquadLink Lite</h1>
<p>{intro}</p>
<p class="muted">{codelbl}</p>
<p class="code">{code}</p>
<ol>
<li><a href="{base}/download/">{step1}</a></li>
<li>{step2}</li>
</ol>
<p><a class="dl" href="{base}/download/">{btn}</a></p>
<p class="muted">{foot}</p>"#
    )
}

// ── Long page bodies kept as constants for readability ───────────────────────

const PRIVACY_EN: &str = r#"<h1>Privacy</h1>
<p class="muted">RDOC SquadLink Lite is built for data minimisation.</p>
<h2>What does NOT happen</h2>
<ul>
<li><b>No audio/chat recording.</b> Voice and text are peer-to-peer (DTLS-SRTP / encrypted DataChannel) and are stored nowhere.</li>
<li><b>No accounts</b>, no login, no tracking, no ads, no cookies.</li>
<li>The brokering server <b>never sees media</b> — voice/chat never pass through it.</li>
</ul>
<h2>What is processed</h2>
<ul>
<li><b>Signaling</b>: to connect, the apps exchange connection data via the server (SDP/ICE candidates, chosen display name, room/session mapping). This lives only <b>in memory</b> and is dropped once the room is empty (within 24&nbsp;h at most).</li>
<li><b>Session brokering</b>: a random code + 6-digit PIN are held temporarily in memory (max 24&nbsp;h) to let mates join without configuration.</li>
<li><b>Connection metadata</b>: like any internet service the server technically sees IP addresses on connect; they are not persistently logged.</li>
<li><b>TURN relay (fallback only)</b>: if no direct path is possible, encrypted audio may pass through a relay. It forwards only <b>encrypted bytes</b> and cannot decrypt them.</li>
</ul>
<h2>Third parties</h2>
<p>Installers are served via GitHub Releases (GitHub's privacy terms apply to the download). STUN/TURN may use public STUN servers for NAT discovery.</p>
<h2>Contact</h2>
<p>Controller: see the <a href="/legal?lang=en">legal notice</a>. Requests via <a href="https://raumdock.org">raumdock.org</a>.</p>"#;

const PRIVACY_DE: &str = r#"<h1>Datenschutzerklärung</h1>
<p class="muted">RDOC SquadLink Lite ist auf Datensparsamkeit ausgelegt.</p>
<h2>Was NICHT passiert</h2>
<ul>
<li><b>Keine Audio-/Chat-Aufzeichnung.</b> Sprache und Text laufen Peer-to-Peer (DTLS-SRTP bzw. verschlüsselter DataChannel) und werden nirgends gespeichert.</li>
<li><b>Keine Benutzerkonten</b>, kein Login, kein Tracking, keine Werbung, keine Cookies.</li>
<li>Der Vermittlungsserver <b>sieht den Medieninhalt nicht</b> — Stimme/Chat fließen nie über ihn.</li>
</ul>
<h2>Was verarbeitet wird</h2>
<ul>
<li><b>Signaling</b>: Beim Verbinden tauschen die Apps über den Server Verbindungsdaten aus (SDP/ICE-Kandidaten, Anzeigename, Raum-/Session-Zuordnung). Diese liegen nur <b>flüchtig im Arbeitsspeicher</b> und werden gelöscht, sobald der Raum leer ist (spätestens nach 24&nbsp;h).</li>
<li><b>Session-Vermittlung</b>: Ein zufälliger Code + 6-stellige PIN werden temporär im Speicher gehalten (max. 24&nbsp;h).</li>
<li><b>Verbindungs-Metadaten</b>: Wie bei jedem Internetdienst sind dem Server beim Verbinden IP-Adressen technisch bekannt; sie werden nicht dauerhaft protokolliert.</li>
<li><b>TURN-Relay (nur Fallback)</b>: Falls keine direkte Verbindung möglich ist, kann verschlüsselter Audioverkehr über einen Relay laufen. Der Relay leitet nur <b>verschlüsselte Bytes</b> weiter.</li>
</ul>
<h2>Drittanbieter</h2>
<p>Installer werden über GitHub Releases bereitgestellt (beim Download gelten die Bestimmungen von GitHub). STUN/TURN kann öffentliche STUN-Server zur NAT-Erkennung nutzen.</p>
<h2>Kontakt</h2>
<p>Verantwortlich: siehe <a href="/legal?lang=de">Impressum</a>. Anfragen über <a href="https://raumdock.org">raumdock.org</a>.</p>"#;

const PRIVACY_IT: &str = r#"<h1>Privacy</h1>
<p class="muted">RDOC SquadLink Lite è progettato per la minimizzazione dei dati.</p>
<h2>Cosa NON accade</h2>
<ul>
<li><b>Nessuna registrazione audio/chat.</b> Voce e testo sono peer-to-peer (DTLS-SRTP / DataChannel cifrato) e non vengono memorizzati da nessuna parte.</li>
<li><b>Nessun account</b>, nessun login, nessun tracciamento, nessuna pubblicità, nessun cookie.</li>
<li>Il server di intermediazione <b>non vede i contenuti multimediali</b>.</li>
</ul>
<h2>Cosa viene trattato</h2>
<ul>
<li><b>Signaling</b>: per connettersi, le app scambiano dati di connessione tramite il server (candidati SDP/ICE, nome visualizzato, associazione stanza/sessione). Restano solo <b>in memoria</b> e vengono eliminati quando la stanza è vuota (entro 24&nbsp;h).</li>
<li><b>Intermediazione sessione</b>: un codice casuale + PIN di 6 cifre sono tenuti temporaneamente in memoria (max 24&nbsp;h).</li>
<li><b>Metadati di connessione</b>: come ogni servizio internet, gli indirizzi IP sono tecnicamente noti alla connessione; non vengono registrati in modo persistente.</li>
<li><b>Relay TURN (solo fallback)</b>: se non è possibile una via diretta, l'audio cifrato può passare per un relay, che inoltra solo <b>byte cifrati</b>.</li>
</ul>
<h2>Terze parti</h2>
<p>Gli installer sono distribuiti via GitHub Releases. STUN/TURN può usare server STUN pubblici per il rilevamento NAT.</p>
<h2>Contatto</h2>
<p>Titolare: vedi <a href="/legal?lang=it">note legali</a>. Richieste tramite <a href="https://raumdock.org">raumdock.org</a>.</p>"#;

const PRIVACY_ES: &str = r#"<h1>Privacidad</h1>
<p class="muted">RDOC SquadLink Lite está diseñado para minimizar los datos.</p>
<h2>Lo que NO ocurre</h2>
<ul>
<li><b>Sin grabación de audio/chat.</b> La voz y el texto son peer-to-peer (DTLS-SRTP / DataChannel cifrado) y no se almacenan en ningún sitio.</li>
<li><b>Sin cuentas</b>, sin inicio de sesión, sin seguimiento, sin anuncios, sin cookies.</li>
<li>El servidor de intermediación <b>nunca ve el contenido multimedia</b>.</li>
</ul>
<h2>Qué se procesa</h2>
<ul>
<li><b>Señalización</b>: para conectar, las apps intercambian datos de conexión a través del servidor (candidatos SDP/ICE, nombre mostrado, asignación de sala/sesión). Solo permanecen <b>en memoria</b> y se eliminan cuando la sala queda vacía (en 24&nbsp;h como máximo).</li>
<li><b>Intermediación de sesión</b>: un código aleatorio + PIN de 6 dígitos se guardan temporalmente en memoria (máx. 24&nbsp;h).</li>
<li><b>Metadatos de conexión</b>: como cualquier servicio de internet, las direcciones IP se conocen técnicamente al conectar; no se registran de forma persistente.</li>
<li><b>Relay TURN (solo respaldo)</b>: si no hay ruta directa, el audio cifrado puede pasar por un relay, que solo reenvía <b>bytes cifrados</b>.</li>
</ul>
<h2>Terceros</h2>
<p>Los instaladores se sirven vía GitHub Releases. STUN/TURN puede usar servidores STUN públicos para la detección de NAT.</p>
<h2>Contacto</h2>
<p>Responsable: ver <a href="/legal?lang=es">aviso legal</a>. Solicitudes vía <a href="https://raumdock.org">raumdock.org</a>.</p>"#;

const PRIVACY_FR: &str = r#"<h1>Confidentialité</h1>
<p class="muted">RDOC SquadLink Lite est conçu pour la minimisation des données.</p>
<h2>Ce qui n'arrive PAS</h2>
<ul>
<li><b>Aucun enregistrement audio/chat.</b> La voix et le texte sont pair-à-pair (DTLS-SRTP / DataChannel chiffré) et ne sont stockés nulle part.</li>
<li><b>Aucun compte</b>, pas de connexion, pas de suivi, pas de publicité, pas de cookies.</li>
<li>Le serveur d'intermédiation <b>ne voit jamais le contenu multimédia</b>.</li>
</ul>
<h2>Ce qui est traité</h2>
<ul>
<li><b>Signalisation</b> : pour se connecter, les apps échangent des données de connexion via le serveur (candidats SDP/ICE, nom affiché, association salle/session). Elles restent uniquement <b>en mémoire</b> et sont supprimées dès que la salle est vide (sous 24&nbsp;h au plus).</li>
<li><b>Intermédiation de session</b> : un code aléatoire + PIN à 6 chiffres sont conservés temporairement en mémoire (max 24&nbsp;h).</li>
<li><b>Métadonnées de connexion</b> : comme tout service internet, les adresses IP sont techniquement connues à la connexion ; elles ne sont pas journalisées durablement.</li>
<li><b>Relais TURN (repli uniquement)</b> : si aucune voie directe n'est possible, l'audio chiffré peut transiter par un relais, qui ne relaie que des <b>octets chiffrés</b>.</li>
</ul>
<h2>Tiers</h2>
<p>Les installeurs sont distribués via GitHub Releases. STUN/TURN peut utiliser des serveurs STUN publics pour la découverte NAT.</p>
<h2>Contact</h2>
<p>Responsable : voir les <a href="/legal?lang=fr">mentions légales</a>. Demandes via <a href="https://raumdock.org">raumdock.org</a>.</p>"#;

const LEGAL_EN: &str = r#"<h1>Legal notice</h1>
<p>RDOC SquadLink Lite is a non-commercial community project (<a href="https://raumdock.org">raumdock.org</a>).</p>
<h2>Authors</h2>
<p>head87x &amp; justcallmedeimos</p>
<h2>Provider</h2>
<p class="muted">Operator: raumdock.org<br>Contact: via <a href="https://raumdock.org">raumdock.org</a></p>
<h2>Liability</h2>
<p>The software is provided "as is", without warranty or liability (see the <a href="/license?lang=en">license</a>). The operators of linked external sites are responsible for their content.</p>"#;

const LEGAL_DE: &str = r#"<h1>Impressum / Rechtliches</h1>
<p>RDOC SquadLink Lite ist ein nicht-kommerzielles Community-Projekt (<a href="https://raumdock.org">raumdock.org</a>).</p>
<h2>Autoren</h2>
<p>head87x &amp; justcallmedeimos</p>
<h2>Anbieter</h2>
<p class="muted"><!-- TODO: vollständige Anbieterkennzeichnung gemäß §5 DDG eintragen -->Verantwortlicher Betreiber: raumdock.org<br>Kontakt: über <a href="https://raumdock.org">raumdock.org</a></p>
<h2>Haftung</h2>
<p>Die Software wird „wie besehen", ohne Gewähr und ohne Haftung bereitgestellt (siehe <a href="/license?lang=de">Lizenz</a>). Für Inhalte verlinkter externer Seiten sind deren Betreiber verantwortlich.</p>"#;

const LEGAL_IT: &str = r#"<h1>Note legali</h1>
<p>RDOC SquadLink Lite è un progetto di community non commerciale (<a href="https://raumdock.org">raumdock.org</a>).</p>
<h2>Autori</h2>
<p>head87x &amp; justcallmedeimos</p>
<h2>Fornitore</h2>
<p class="muted">Operatore: raumdock.org<br>Contatto: tramite <a href="https://raumdock.org">raumdock.org</a></p>
<h2>Responsabilità</h2>
<p>Il software è fornito "così com'è", senza garanzie né responsabilità (vedi <a href="/license?lang=it">licenza</a>). I gestori dei siti esterni collegati sono responsabili dei loro contenuti.</p>"#;

const LEGAL_ES: &str = r#"<h1>Aviso legal</h1>
<p>RDOC SquadLink Lite es un proyecto comunitario no comercial (<a href="https://raumdock.org">raumdock.org</a>).</p>
<h2>Autores</h2>
<p>head87x &amp; justcallmedeimos</p>
<h2>Proveedor</h2>
<p class="muted">Operador: raumdock.org<br>Contacto: vía <a href="https://raumdock.org">raumdock.org</a></p>
<h2>Responsabilidad</h2>
<p>El software se ofrece "tal cual", sin garantía ni responsabilidad (ver <a href="/license?lang=es">licencia</a>). Los operadores de los sitios externos enlazados son responsables de su contenido.</p>"#;

const LEGAL_FR: &str = r#"<h1>Mentions légales</h1>
<p>RDOC SquadLink Lite est un projet communautaire non commercial (<a href="https://raumdock.org">raumdock.org</a>).</p>
<h2>Auteurs</h2>
<p>head87x &amp; justcallmedeimos</p>
<h2>Fournisseur</h2>
<p class="muted">Exploitant : raumdock.org<br>Contact : via <a href="https://raumdock.org">raumdock.org</a></p>
<h2>Responsabilité</h2>
<p>Le logiciel est fourni « en l'état », sans garantie ni responsabilité (voir la <a href="/license?lang=fr">licence</a>). Les exploitants des sites externes liés sont responsables de leur contenu.</p>"#;
