import { useState } from "react";
import { Link } from "react-router-dom";

type WikiArticle = Readonly<{
  id: string;
  section: string;
  title: string;
  summary: string;
  points: readonly string[];
  links?: readonly { label: string; to: string }[];
}>;

const articles: readonly WikiArticle[] = [
  {
    id: "start",
    section: "Erste Schritte",
    title: "Was lxcup verwaltet",
    summary: "lxcup ist die zentrale Oberfläche für vorhandene Systeme und freigegebene Betriebsaufgaben.",
    points: [
      "Verwalte vorhandene Linux-Server, LXC-Container und Windows-Systeme. lxcup erstellt keine virtuellen Maschinen oder LXC-Gäste.",
      "Ein Agent meldet sich selbst beim Controller. Nach dem Einbinden erscheinen Agent-Version, Heartbeat, Paketbestand und – sofern unterstützt – Telemetrie.",
      "Docker-Container sind Inventar auf einem verwalteten Linux-Host, keine eigenständigen Zielsysteme.",
      "Die linke Navigation führt zu Ressourcen, Automatisierung, Systemverwaltung und diesem Wiki.",
    ],
    links: [
      { label: "Übersicht", to: "/" },
      { label: "Server einbinden", to: "/servers" },
      { label: "LXC-Container", to: "/containers" },
      { label: "Windows-Systeme", to: "/windows" },
    ],
  },
  {
    id: "konto",
    section: "Konto und Zugriff",
    title: "Anmelden und Rollen",
    summary: "lxcup verwendet lokale Benutzerkonten ohne öffentliche Registrierung oder E-Mail-Adresse.",
    points: [
      "Beim ersten Start wird das Konto admin mit dem temporären Passwort admin angelegt. Ändere dieses Passwort direkt bei der ersten Anmeldung.",
      "Ein Benutzerkonto kann sein eigenes Passwort über Passwort ändern wechseln. Passwörter müssen mindestens 12 Zeichen lang sein.",
      "User können Ressourcen und freigegebene, nicht-destruktive Workflows verwenden. Admins verwalten zusätzlich Konten und Secrets, lesen das Aktivitätsprotokoll und dürfen bestätigungspflichtige destruktive Aktionen ausführen.",
      "Rollen- und Passwortänderungen beenden bestehende Sitzungen des betroffenen Kontos. Deaktivierte Konten können sich nicht anmelden.",
    ],
    links: [
      { label: "Passwort ändern", to: "/account/security" },
      { label: "Benutzerverwaltung (Admin)", to: "/admin/users" },
    ],
  },
  {
    id: "ressourcen",
    section: "Ressourcen",
    title: "Ressourcen einbinden und verwalten",
    summary: "Registriere Zugangsdaten, installiere den passenden Agenten und verfolge den Onboarding-Fortschritt.",
    points: [
      "Für ein Linux-Ziel werden Adresse, SSH-Benutzer und Secret-Referenzen für den Zugang, den Agenten und den SSH-Hostschlüssel hinterlegt. Das Host-Vorbereitungsskript kann als root oder über sudo ausgeführt werden.",
      "Einen bereits entdeckten, laufenden LXC kannst du über LXC aufnehmen mit einem Zugangsprofil verknüpfen. Optional installiert lxcup den Agenten und startet nach erfolgreicher Verbindung Healthcheck und Paketinventar.",
      "Windows-Systeme benötigen vorbereitete WinRM-Zugangsdaten. Die verfügbaren Schritte und Statusmeldungen zeigt der jeweilige Einbindungsbereich.",
      "Nach erfolgreicher Agent-Installation und Verbindung folgen Healthcheck und Paketinventar. Nicht verbundene oder veraltete Agenten werden in der Ressourcenübersicht markiert.",
      "Die Ressourcenkarte öffnet die Detailansicht mit Inventar, Verlauf und – für unterstützte Linux-Ziele – Docker-Erkennung.",
      "Ein Admin kann ein Ziel nach Bestätigung aus lxcup entfernen. Dabei werden zugehörige Controller-Daten entfernt; der Agent auf dem Host wird nicht deinstalliert und Secret-Einträge bleiben erhalten.",
    ],
    links: [
      { label: "Linux-Server", to: "/servers" },
      { label: "LXC-Container", to: "/containers" },
      { label: "LXC aufnehmen", to: "/enrollments/new" },
      { label: "Windows-Systeme", to: "/windows" },
    ],
  },
  {
    id: "pakete",
    section: "Inventar und Telemetrie",
    title: "Paketinventar und Updates",
    summary: "Das Paketinventar zeigt installierte Versionen und – sofern verfügbar – die Kandidatenversion.",
    points: [
      "Öffne das Paketinventar aus der jeweiligen Ressource. Suche und sortiere die Pakete; eine Inventarerfassung ist eine lesende Workflow-Aufgabe.",
      "Für ein einmaliges Update brauchst du keinen Zeitplan. Ein Apply benötigt aber eine passende Update-Policy und einen erfolgreichen Plan für dasselbe Ziel und denselben Paketumfang.",
      "Im Paket-Auswahlfenster kannst du gezielt verfügbare Updates markieren. Eine leere Auswahl bedeutet: alle verfügbaren Updates dieses Ziels.",
      "Paketänderungen laufen sicherheitsbewusst als Prüfen/Planen, ausdrückliche Bestätigung und Apply. Ein Apply verlangt weiterhin eine ausdrückliche Bestätigung.",
      "Die Standard-Policy erlaubt alle Pakete und hat ein ganztägiges UTC-Wartungsfenster. Sie ist systemweit geteilt und kann nicht gelöscht werden.",
    ],
    links: [
      { label: "Alle Workflows", to: "/workflows" },
      { label: "Update-Policies", to: "/update-policies" },
    ],
  },
  {
    id: "telemetrie",
    section: "Inventar und Telemetrie",
    title: "Systemauslastung und Benachrichtigungen",
    summary: "Linux-Agenten liefern CPU-, RAM- und Speicherwerte; die Detailseite zeigt den jüngsten Verlauf.",
    points: [
      "Die Telemetrieansicht zeigt die letzten zehn Minuten. Linux-Agenten erfassen Messwerte regelmäßig und übertragen überlappende Zeitfenster, damit verspätete Heartbeats Lücken auffüllen können.",
      "Ein Alert wird bei anhaltender Auslastung ausgelöst: CPU über 90 % für 5 Minuten (kritisch über 98 % für 2 Minuten), RAM über 90 % für 5 Minuten (kritisch über 95 % für 2 Minuten) und Speicher über 85 % für 10 Minuten (kritisch über 95 % für 5 Minuten).",
      "Fehlende Messpunkte unterbrechen die Dauerbewertung. Bleiben Telemetriedaten länger als zwei Minuten aus, erscheint ein Aktualitätsalarm.",
      "Die Benachrichtigungen oben in der Anwendung zeigen aktive und behobene Telemetriealarme sowie fehlgeschlagene Workflows. Ein Klick öffnet die betroffene Ressource oder das Workflow-Protokoll.",
    ],
    links: [{ label: "Ressourcenübersicht", to: "/" }],
  },
  {
    id: "docker",
    section: "Container",
    title: "Docker-Container entdecken und verwalten",
    summary: "Die Docker-Erkennung läuft über den verbundenen Agenten eines Linux-Servers oder LXC-Ziels.",
    points: [
      "Öffne Docker-Container, wähle einen verbundenen Linux-Host und starte die Erkennung. Windows-Ziele sind hierfür nicht unterstützt.",
      "Der Host braucht Docker und der Agent muss auf den Docker-Socket zugreifen können. Auf Linux kann die Vorbereitung den Agenten zur docker-Gruppe hinzufügen; diese Berechtigung entspricht praktisch Root-Zugriff auf dem Host.",
      "Das Inventar gruppiert Container nach Host und Compose-Projekt. Der Projektfilter hilft, eine Compose-Anwendung einzugrenzen.",
      "Aufnehmen und Entfernen ändern nur den lxcup-Inventareintrag, nicht den Container selbst. Start, Stop und Neustart verändern dagegen den Container und verlangen eine Bestätigung sowie Admin-Berechtigung.",
      "Image-Updates lassen sich prüfen. Ein tatsächliches Update ist nur für unterstützte, Compose-verwaltete Linux-Services mit einer einzelnen Instanz möglich; lxcup erstellt dabei kein automatisches Rollback.",
      "Die Docker-Erkennung kann manuell für Linux-Server und LXC gestartet werden. Als Zeitplan-Aufgabe steht sie derzeit nur für LXC-Ziele zur Verfügung.",
    ],
    links: [
      { label: "Docker-Inventar", to: "/docker" },
      { label: "Linux-Server", to: "/servers" },
      { label: "LXC-Container", to: "/containers" },
    ],
  },
  {
    id: "workflows",
    section: "Automatisierung",
    title: "Einzelne und gebündelte Workflows",
    summary: "Workflows starten freigegebene Aufgaben; beliebige Shell-Befehle oder Playbooks werden nicht eingegeben.",
    points: [
      "Einzelner Workflow bearbeitet ein gewähltes Ziel. Bulk-Workflows reihen dieselbe Operation für mehrere ausgewählte Ziele ein; lxcup erstellt je Ziel einen eigenen Job.",
      "Je nach Aufgabe stehen Check, Plan oder Apply zur Verfügung. Lies vor dem Start Ziel, Operation, Modus und Bestätigungshinweis sorgfältig.",
      "Pro Ziel kann nur ein Job zur selben Zeit aktiv sein. Warte, bis der aktive Job beendet ist, bevor du weitere Arbeit für dieses Ziel startest.",
      "Das Protokoll zeigt Status, Zeitverlauf, Aufgaben und technische Ausgaben. Nach einem unterbrochenen Apply muss der Ist-Zustand abgeglichen werden, bevor weitere Änderungen ausgeführt werden.",
      "Wenn Controller-Worker oder Artifact Store nicht bereit sind, zeigt lxcup den gemeldeten Verfügbarkeitsstatus. Agent-Artefakte müssen zur unterstützten Architektur passen.",
    ],
    links: [
      { label: "Workflow starten", to: "/workflows" },
      { label: "Aktivität", to: "/workflows" },
    ],
  },
  {
    id: "zeitplaene",
    section: "Automatisierung",
    title: "Zeitpläne und Update-Policies",
    summary: "Zeitpläne wiederholen eine freigegebene Aufgabe; Policies begrenzen, welche Paketupdates zulässig sind.",
    points: [
      "Ein Zeitplan verbindet Namen, Ziel, Aufgabe und Wiederholungsintervall. Die Zeitzone bestimmt die Berechnung der nächsten Ausführung.",
      "Für unterstützte Aufgaben kann eine Ausführungsbedingung an einen Telemetrie-Schwellwert geknüpft werden. Ohne Bedingung läuft die Aufgabe nach dem Intervall.",
      "Ein Paketupdate-Zeitplan benötigt eine aktive Policy. Ein einzelner Paketupdate-Workflow braucht keinen Zeitplan, aber für Apply weiterhin Policy, passenden erfolgreichen Plan und Bestätigung.",
      "Eine Policy beschränkt ein Ziel, erlaubte Paketnamen (kommagetrennt; leer erlaubt alle Pakete), maximales Risiko und UTC-Wartungsfenster.",
      "Die gemeinsam verwendete System-Policy lxcup-standard-all-packages erlaubt alle Pakete bei hohem Maximalrisiko und 00:00–23:59 UTC. Sie kann nicht gelöscht werden.",
    ],
    links: [
      { label: "Zeitpläne", to: "/schedules" },
      { label: "Update-Policies", to: "/update-policies" },
    ],
  },
  {
    id: "admin",
    section: "Administration",
    title: "Secrets, Benutzer und Aktivitätsprotokoll",
    summary: "Administrative Funktionen sind nur für Admins sichtbar und werden zusätzlich serverseitig geschützt.",
    points: [
      "In Secrets verwaltest du Zugangsdaten, SSH-Hostschlüssel und Agent-Tokens. Secret-Werte werden nach dem Speichern nicht wieder angezeigt; rotiere oder widerrufe sie über die vorgesehenen Aktionen.",
      "In der Benutzerverwaltung kannst du lokale Konten anlegen, Rollen ändern, Konten deaktivieren, temporäre Passwörter zurücksetzen und Benutzer löschen. Neue oder zurückgesetzte Passwörter müssen beim Login geändert werden.",
      "Das Aktivitätsprotokoll zeigt, wer welche unterstützte Aktion an welchem Objekt ausgeführt hat. Es ist nur für Admins zugänglich; Passwort- und Secret-Werte gehören nicht in Protokolle.",
      "Das letzte aktive Admin-Konto kann nicht herabgestuft, deaktiviert oder gelöscht werden.",
    ],
    links: [
      { label: "Secrets", to: "/secrets" },
      { label: "Benutzerverwaltung", to: "/admin/users" },
      { label: "Aktivitätsprotokoll", to: "/admin/audit" },
    ],
  },
  {
    id: "terminal",
    section: "Administration",
    title: "SSH-Terminal für Linux und LXC",
    summary: "Admins können aus der Ressourcendetailseite eine interaktive SSH-Sitzung öffnen.",
    points: [
      "Das Terminal steht nur Admins für aktivierte Linux-Server und LXC-Ziele mit SSH-Transport zur Verfügung. Benutzerkonten mit erzwungenem Passwortwechsel können es noch nicht öffnen.",
      "lxcup verbindet sich mit Adresse, SSH-Benutzer, Zugangsdaten und bekanntem Hostschlüssel aus dem gespeicherten Zielprofil. Zugangsdaten werden nicht im Browser eingegeben; ein unbekannter oder geänderter Hostschlüssel wird abgelehnt.",
      "Die Sitzung öffnet eine interaktive Shell auf dem Ziel. Eingaben und Ausgaben werden weder gespeichert noch ins Aktivitätsprotokoll geschrieben; dort erscheinen nur Sitzungsstart und -ende.",
      "Pro Ziel ist eine Sitzung gleichzeitig möglich; ein Admin kann höchstens zwei Sitzungen parallel öffnen. Nach 10 Minuten ohne Aktivität oder spätestens nach 60 Minuten wird die Verbindung beendet.",
      "Der Controller benötigt ausgehenden SSH-Zugriff auf das Ziel. Prüfe bei Verbindungsfehlern Zieladresse, SSH-Erreichbarkeit, aktive Credential- und Known-Hosts-Secrets sowie den registrierten Benutzer.",
    ],
    links: [
      { label: "Linux-Server", to: "/servers" },
      { label: "LXC-Container", to: "/containers" },
      { label: "Aktivitätsprotokoll", to: "/admin/audit" },
    ],
  },
  {
    id: "hilfe",
    section: "Hilfe und Fehlerbehebung",
    title: "Häufige Statusmeldungen verstehen",
    summary: "Prüfe zuerst Ressource, Jobprotokoll und Verbindungsstatus, bevor du einen Lauf erneut startest.",
    points: [
      "Agent nicht verbunden oder Telemetrie veraltet: Prüfe Agent-Dienst, Controller-Adresse, Agent-Version und letzten Heartbeat auf der Ressourcendetailseite.",
      "Docker nicht verfügbar: Prüfe, ob Docker auf dem Linux-Ziel läuft und der Agent auf /var/run/docker.sock zugreifen darf. Eine Docker-Installation allein reicht ohne Socket-Berechtigung nicht.",
      "another job is active for this target: Für dieses Ziel läuft noch ein anderer Job oder sein Ergebnis ist noch nicht abgeglichen. Öffne die Jobliste und warte auf Abschluss beziehungsweise führe den angeforderten Abgleich aus.",
      "Worker oder Artifact Store nicht verfügbar: Ein Workflow benötigt einen verfügbaren Worker; Agent-Installations- und Updatejobs benötigen außerdem passende Artefakte. Prüfe die Verfügbarkeitsmeldung und das Jobprotokoll.",
      "Fehlgeschlagene Jobs nicht blind wiederholen: Lies den Fehler und die bereits vorgenommenen Änderungen. Bei Abgleich erforderlich erst Ist-Zustand abgleichen, dann den nächsten Schritt festlegen.",
    ],
    links: [
      { label: "Workflow-Protokolle", to: "/workflows" },
      { label: "Ressourcen", to: "/servers" },
    ],
  },
];

export function WikiPage({ isAdmin = false }: Readonly<{ isAdmin?: boolean }>) {
  const [search, setSearch] = useState("");
  const searchTerms = search.trim().toLocaleLowerCase("de").split(/\s+/).filter(Boolean);
  const visibleArticles = searchTerms.length > 0
    ? articles.filter((article) => {
      const content = [article.section, article.title, article.summary, ...article.points].join(" ").toLocaleLowerCase("de");
      return searchTerms.every((term) => content.includes(term));
    })
    : articles;
  const sections = Array.from(new Set(visibleArticles.map((article) => article.section)));

  return <div className="grid gap-5 text-[var(--ink)]">
    <header className="grid items-center gap-5 border border-[var(--line)] bg-[var(--panel)] p-5 lg:grid-cols-[minmax(0,1fr)_minmax(16rem,24rem)]">
      <div><p className="mb-2 text-[10px] font-bold uppercase tracking-[0.14em] text-lxcup-primary">Hilfe · Funktionswiki</p><h1 className="mb-2 text-2xl font-semibold tracking-tight">lxcup-Wiki</h1><p className="mb-0 max-w-2xl text-sm leading-relaxed text-[var(--muted)]">Anleitungen und Erklärungen zu den Funktionen, die du in lxcup verwenden kannst.</p></div>
      <label className="grid min-w-0 gap-2 text-xs font-semibold text-[var(--muted)]">Wiki durchsuchen<input type="search" className="h-10 w-full" value={search} onChange={(event) => setSearch(event.target.value)} placeholder="Docker, Telemetrie, Passwort …" /></label>
    </header>

    <div className="grid items-start gap-5 lg:grid-cols-[15rem_minmax(0,1fr)]">
      <aside className="grid gap-4 border border-[var(--line)] bg-[var(--panel)] p-4 lg:sticky lg:top-3">
        <div className="flex items-center justify-between border-b border-[var(--line)] pb-3"><h2 className="m-0 text-xs font-bold uppercase tracking-wider">Themen</h2><span className="text-xs text-[var(--muted)]">{visibleArticles.length} Artikel</span></div>
        <nav aria-label="Wiki-Inhaltsverzeichnis" className="grid gap-1 min-[380px]:grid-cols-2 lg:grid-cols-1">
          {sections.map((section, index) => <a className="group flex min-h-10 items-center gap-3 border border-transparent px-2 py-2 text-xs font-medium text-[var(--muted)] transition-colors hover:border-[var(--line)] hover:bg-[var(--primary-soft)] hover:text-lxcup-primary focus-visible:outline focus-visible:outline-2 focus-visible:outline-[var(--primary)]" key={section} href={`#${sectionId(section)}`}><span className="text-[10px] font-semibold tabular-nums text-lxcup-primary">{String(index + 1).padStart(2, "0")}</span><span className="min-w-0 flex-1">{section}</span><WikiArrow /></a>)}
        </nav>
        <p className="m-0 border-t border-[var(--line)] pt-3 text-xs leading-relaxed text-[var(--muted)]">Wähle ein Thema oder suche nach einer Funktion.</p>
      </aside>

      <div className="grid min-w-0 gap-6">
        {visibleArticles.length === 0 ? <div className="grid justify-items-center gap-3 border border-dashed border-[var(--line)] bg-[var(--panel)] p-8 text-center"><p className="m-0 text-sm text-[var(--muted)]">Keine Wiki-Themen für „{search}“ gefunden. Prüfe den Suchbegriff oder lösche die Suche.</p><button type="button" className="min-h-9 border border-[var(--line)] px-3 text-xs font-semibold text-lxcup-primary hover:bg-[var(--primary-soft)]" onClick={() => setSearch("")}>Suche zurücksetzen</button></div> : null}
        {sections.map((section) => <section className="grid min-w-0 scroll-mt-3 gap-3" aria-labelledby={`${sectionId(section)}-heading`} id={sectionId(section)} key={section}>
          <h2 className="m-0 text-[10px] font-bold uppercase tracking-[0.14em] text-[var(--muted)]" id={`${sectionId(section)}-heading`}>{section}</h2>
          {visibleArticles.filter((article) => article.section === section).map((article) => <WikiArticleCard key={article.id} article={article} isAdmin={isAdmin} />)}
        </section>)}
        <p className="m-0 border-l-2 border-l-lxcup-primary bg-[var(--primary-soft)] px-4 py-3 text-xs leading-relaxed text-[var(--muted)]">Verfügbare Aktionen hängen von Ressourcentyp, Agent-Status, Rolle und serverseitigen Freigaben ab.</p>
      </div>
    </div>
  </div>;
}

function WikiArticleCard({ article, isAdmin }: Readonly<{ article: WikiArticle; isAdmin: boolean }>) {
  const links = article.links?.filter((link) => isAdmin || !["/secrets", "/admin/users", "/admin/audit"].includes(link.to)) ?? [];
  return <article className="overflow-hidden border border-[var(--line)] bg-[var(--panel)]">
    <header className="border-b border-[var(--line)] p-5"><h3 className="mb-2 text-lg font-semibold tracking-tight">{article.title}</h3><p className="mb-0 max-w-[85ch] text-sm leading-relaxed text-[var(--muted)]">{article.summary}</p></header>
    <div className={links.length > 0 ? "grid min-w-0 2xl:grid-cols-[minmax(0,1fr)_14rem]" : "min-w-0"}>
      <ul className="m-0 grid list-none content-start gap-4 p-5 text-sm leading-7">{article.points.map((point) => <li className="flex items-start gap-3" key={point}><span className="mt-3 h-1 w-1 shrink-0 rounded-full bg-lxcup-primary" aria-hidden="true" /><span className="max-w-[90ch]">{point}</span></li>)}</ul>
      {links.length > 0 ? <div className="grid content-start gap-2 border-t border-[var(--line)] bg-[var(--paper-muted)] p-5 2xl:border-l 2xl:border-t-0"><p className="mb-1 text-[10px] font-bold uppercase tracking-wider text-[var(--muted)]">Direkt öffnen</p>{links.map((link) => <Link className="flex min-h-9 items-center justify-between gap-3 border border-[var(--line)] bg-[var(--panel)] px-3 py-2 text-xs font-semibold text-lxcup-primary transition-colors hover:border-[var(--primary)] hover:bg-[var(--primary-soft)]" key={link.to + link.label} to={link.to}><span>{link.label}</span><WikiArrow /></Link>)}</div> : null}
    </div>
  </article>;
}

function WikiArrow() {
  return <svg className="h-3.5 w-3.5 shrink-0" viewBox="0 0 16 16" fill="none" stroke="currentColor" strokeWidth="1.5" strokeLinecap="round" strokeLinejoin="round" aria-hidden="true"><path d="M3 8h10M9 4l4 4-4 4" /></svg>;
}

function sectionId(section: string) {
  return `wiki-${section.toLocaleLowerCase("de").replaceAll(/[^a-z0-9]+/g, "-").replaceAll(/(^-|-$)/g, "")}`;
}
