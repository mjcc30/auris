import { useCallback, useEffect, useRef, useState } from "react";

import { api, dopCarrier, fmtBytes, fmtMix, fmtTime, makeSource, type AlarmsInfo, type Album, type AppearanceSettings, type Artist, type ArtistDetail, type AudioDevice, type BrowseEntry, type CamillaSettings, type CamillaStatusResponse, type CopyReport, type DopStatus, type Facets, type FavKind, type Favorites, type FavoritesResolved, type HistoryTrack, type InfosResponse, type InputDevice, type LibraryStats, type LyricsResponse, type MultiroomState, type MrGroup, type NetInterface, type PinKind, type Pins, type MountInfo, type Packs, type Playlist, type PluginInfo, type PlaylistDetail, type PlayerState, type PodcastEpisode, type PodcastFeed, type PodcastShow, type RadioEntry, type RadioList, type RipStatus, type DirCountry, type DirSection, type DirStation, type DirTag, type SearchResults, type Settings, type ShareLink, type SignalPath, type SmartField, type SmartOp, type SmartRule, type SmartRules, type Source, type SpotifySettings, type SpotStatus, type StorageInfo, type SystemInfo, type Track, type UpnpDevice } from "./api";
import { Icon } from "./Icon";
import { BrowserAudio } from "./browserAudio";
import { loadLocalePack, setLocale, t } from "./i18n";

type View = "home" | "library" | "browse" | "radios" | "queue" | "playlists" | "favorites" | "sources" | "multiroom" | "plugins" | "settings" | "display";
type SourceKindTab = "local" | "usb" | "smb" | "radio" | "http" | "upnp" | "qobuz" | "tidal" | "linein" | "archive" | "deezer" | "jamendo" | "youtube";
/** Onglets du panneau lecture en cours (style Volumio: Pochette / Infos / Paroles). */
type NpTab = "cover" | "infos" | "lyrics";

function kindIconName(kind: string): string {
  switch (kind) {
    case "usb": return "usb";
    case "smb": return "server";
    case "upnp": return "server";
    case "radio": return "radio";
    case "http": return "link";
    case "qobuz": return "note";
    case "tidal": return "note";
    case "linein": return "link";
    case "archive": return "globe";
    case "deezer": return "note";
    case "jamendo": return "globe";
    case "youtube": return "play";
    default: return "folder";
  }
}

function kindLabel(kind: string): string {
  switch (kind) {
    case "usb": return "USB";
    case "smb": return "SMB · CIFS";
    case "upnp": return "UPnP/DLNA";
    case "radio": return "Radio";
    case "http": return "HTTP";
    case "qobuz": return "Qobuz";
    case "tidal": return "Tidal";
    case "linein": return "Line-in / optique / HDMI";
    case "archive": return "Internet Archive";
    case "deezer": return "Deezer (extraits 30 s)";
    case "jamendo": return "Jamendo (Creative Commons)";
    case "youtube": return "YouTube (yt-dlp)";
    default: return "Dossier local";
  }
}

function slugify(s: string): string {
  return s
    .toLowerCase()
    .normalize("NFD")
    .replace(/[̀-ͯ]/g, "")
    .replace(/[^a-z0-9]+/g, "-")
    .replace(/^-+|-+$/g, "") || "source";
}

/// Compte traduit avec singulier/pluriel : cl(1, "titre", "titres") → « 1 titre ».
function cl(n: number, one: string, many: string): string {
  return t(`{count} ${n === 1 ? one : many}`, { count: n });
}

/// Assombrit/éclaircit une couleur `#RRGGBB` (amt en [-255, 255]).
/// Presets de couleur d'accent proposés dans les réglages (#72).
const ACCENT_PRESETS = ["#1ed760", "#ff8800", "#ff4d4d", "#2979ff", "#aa00ff", "#00bfa5", "#ffd600", "#f5f5f5"];

function shade(hex: string, amt: number): string {
  const m = /^#?([0-9a-f]{6})$/i.exec(hex.trim());
  if (!m) return hex;
  const n = parseInt(m[1], 16);
  const f = (x: number) => Math.max(0, Math.min(255, x + amt));
  const r = f((n >> 16) & 255), g = f((n >> 8) & 255), b = f(n & 255);
  return `#${((r << 16) | (g << 8) | b).toString(16).padStart(6, "0")}`;
}

/// Applique thème / accent / fond au document (persisté côté backend).
/// Un thème hors dark/light/system est un pack CSS servi par `/api/packs/theme/{id}.css`
/// (embarqué ou déposé dans `<data-dir>/themes/`).
function applyAppearance(a: AppearanceSettings) {
  const root = document.documentElement;
  const theme = a.theme === "system"
    ? (window.matchMedia("(prefers-color-scheme: light)").matches ? "light" : "dark")
    : a.theme;
  if (theme === "dark" || theme === "light") {
    root.dataset.theme = theme;
    document.getElementById("auris-theme-pack")?.remove();
  } else {
    // pack de thème : base sombre, le CSS du pack écrase les variables
    root.dataset.theme = "dark";
    let link = document.getElementById("auris-theme-pack") as HTMLLinkElement | null;
    if (!link) {
      link = document.createElement("link");
      link.id = "auris-theme-pack";
      link.rel = "stylesheet";
      document.head.appendChild(link);
    }
    link.href = `/api/packs/theme/${encodeURIComponent(a.theme)}.css`;
  }
  root.style.setProperty("--accent", a.accent);
  root.style.setProperty("--accent-dark", shade(a.accent, -20));
  if (a.background) {
    document.body.style.backgroundImage = `linear-gradient(rgba(0,0,0,0.55), rgba(0,0,0,0.8)), url("${a.background}")`;
    document.body.style.backgroundSize = "cover";
    document.body.style.backgroundAttachment = "fixed";
  } else {
    document.body.style.backgroundImage = "";
    document.body.style.backgroundSize = "";
    document.body.style.backgroundAttachment = "";
  }
}

export default function App() {
  const [view, setView] = useState<View>("home");
  const [state, setState] = useState<PlayerState | null>(null);
  const [sources, setSources] = useState<Source[]>([]);
  const [devices, setDevices] = useState<AudioDevice[]>([]);
  const [system, setSystem] = useState<SystemInfo | null>(null);
  const [settings, setSettings] = useState<Settings | null>(null);
  const [packs, setPacks] = useState<Packs | null>(null);
  const [favs, setFavs] = useState<Favorites>({ tracks: [], albums: [], artists: [] });
  const [pins, setPins] = useState<Pins>({ albums: [], playlists: [], radios: [] });
  const [stats, setStats] = useState<LibraryStats | null>(null);
  const [, setLocaleTick] = useState(0);
  const [alarms, setAlarms] = useState<AlarmsInfo | null>(null);
  const [browseSrc, setBrowseSrc] = useState<string | null>(null);
  // Serveurs UPnP/DLNA découverts sur le LAN (#11) — chargés en arrière-plan.
  const [upnpDevs, setUpnpDevs] = useState<UpnpDevice[]>([]);
  const [upnpScanning, setUpnpScanning] = useState(false);
  const [toast, setToast] = useState<string | null>(null);
  const [searchNonce, setSearchNonce] = useState(0);
  const [searchQ, setSearchQ] = useState("");
  // Panneau « lecture en cours » (Pochette / Infos / Paroles) à droite de la vue.
  const [npOpen, setNpOpen] = useState(false);
  const [npTab, setNpTab] = useState<NpTab>("cover");
  // Navigation différée vers la bibliothèque (depuis l'accueil) : artiste/album à ouvrir.
  const [pendingArtist, setPendingArtist] = useState<{ id: string; name: string } | null>(null);
  const [pendingAlbum, setPendingAlbum] = useState<{ id: string; title: string; artist: string } | null>(null);
  const toastTimer = useRef<number | null>(null);
  const browserAudio = useRef(new BrowserAudio());

  const say = useCallback((msg: string) => {
    setToast(msg);
    if (toastTimer.current) window.clearTimeout(toastTimer.current);
    toastTimer.current = window.setTimeout(() => setToast(null), 4500);
  }, []);

  const refresh = useCallback(async () => {
    try {
      const [s, srcs, devs, sys, st, pk, fv, libStats, pn] = await Promise.all([
        api.state(), api.sources(), api.devices(), api.system(), api.settings(),
        api.packs().catch(() => null),
        api.favorites().catch(() => null),
        api.libraryStats().catch(() => null),
        api.pins().catch(() => null),
      ]);
      setState(s); setSources(srcs); setDevices(devs); setSystem(sys); setSettings(st); setPacks(pk);
      if (fv) setFavs(fv);
      if (libStats) setStats(libStats);
      if (pn) setPins(pn);
    } catch { /* backend indisponible pour l'instant */ }
  }, []);

  // Découverte SSDP UPnP en arrière-plan (ne bloque pas le premier rendu) — #11.
  const scanUpnp = useCallback(async (force = false) => {
    setUpnpScanning(true);
    try { setUpnpDevs(await api.upnpDiscover(force)); }
    catch { /* SSDP indisponible : la section reste vide */ }
    finally { setUpnpScanning(false); }
  }, []);

  useEffect(() => {
    refresh();
    scanUpnp();
    const ws = new WebSocket(`${location.protocol === "https:" ? "wss" : "ws"}://${location.host}/ws`);
    // Les événements « state » (horloge de position toutes les 500 ms) mettent à jour
    // l'état directement sans requêtes HTTP ; seuls les autres événements rechargent tout.
    ws.onmessage = (e) => {
      try {
        const ev = JSON.parse(String(e.data));
        if (ev?.type === "state" && ev.payload) setState(ev.payload as PlayerState);
        // messages du moteur (erreur flux audio, bascule de sortie, alarmes…): toast + refresh
        else if (ev?.type === "message" && ev.payload?.message) { say(String(ev.payload.message)); refresh(); }
        else if (ev?.type) refresh();
        else setState(ev as PlayerState);
      } catch { refresh(); }
    };
    const iv = setInterval(refresh, 10000);
    return () => { ws.close(); clearInterval(iv); };
  }, [refresh, say, scanUpnp]);

  // Applique l'apparence (thème, accent, fond) au document à chaque changement.
  const appearance = settings?.appearance;
  useEffect(() => {
    if (appearance) applyAppearance(appearance);
    // Dépendances sur les champs primitifs : évite de réécrire les styles du document à chaque message WS.
  }, [appearance?.theme, appearance?.accent, appearance?.background]);

  // Thème « system » : re-applique en live si l'OS change de préférence clair/sombre.
  useEffect(() => {
    const mq = window.matchMedia("(prefers-color-scheme: light)");
    const onChange = () => {
      if (appearance && appearance.theme === "system") applyAppearance(appearance);
    };
    mq.addEventListener("change", onChange);
    return () => mq.removeEventListener("change", onChange);
  }, [appearance]);

  // Langue de l'interface : charge le pack de langue (embarqué ou communautaire) puis re-render.
  const language = appearance?.language ?? "fr";
  useEffect(() => {
    let cancelled = false;
    loadLocalePack(language).then((pack) => {
      if (cancelled) return;
      setLocale(language, pack);
      setLocaleTick((x) => x + 1);
    });
    return () => { cancelled = true; };
  }, [language]);

  // « Jouer ici » : démarre/arrête le lecteur WebAudio selon la sortie active.
  useEffect(() => {
    if (state?.output_mode === "browser") browserAudio.current.start();
    else browserAudio.current.stop();
  }, [state?.output_mode]);

  // Minuterie de sommeil / réveil : sondage doux (5 s) pour le badge du lecteur.
  useEffect(() => {
    let dead = false;
    const load = async () => { try { const a = await api.alarms(); if (!dead) setAlarms(a); } catch { /* ignore */ } };
    load();
    const iv = setInterval(load, 5000);
    return () => { dead = true; clearInterval(iv); };
  }, []);

  // Statut DoP/WASAPI du DAC : on ne sonde que pendant la lecture d'une piste
  // DSD (c'est le seul cas où le réglage du DAC importe pour l'UI).
  const [dop, setDop] = useState<DopStatus | null>(null);
  const [spOpen, setSpOpen] = useState(false);
  const dsdCarrier = dopCarrier(state?.current_format ?? state?.current?.format);
  useEffect(() => {
    if (!dsdCarrier) { setDop(null); return; }
    let dead = false;
    const load = async () => { try { const d = await api.dopStatus(); if (!dead) setDop(d); } catch { /* ignore */ } };
    load();
    const iv = setInterval(load, 15000);
    return () => { dead = true; clearInterval(iv); };
  }, [dsdCarrier]);

  const act = useCallback(async (fn: () => Promise<unknown>, okMsg?: string) => {
    try { await fn(); if (okMsg) say(okMsg); await refresh(); }
    catch (e) { say(e instanceof Error ? e.message : String(e)); }
  }, [refresh, say]);

  // Media Session API — PWA installée / contrôles média du téléphone (#95) :
  // métadonnées du morceau + actions play/pause/précédent/suivant.
  useEffect(() => {
    if (!("mediaSession" in navigator)) return;
    const cur = state?.current ?? null;
    navigator.mediaSession.playbackState =
      !state || state.status === "stopped" ? "none" : state.status === "playing" ? "playing" : "paused";
    navigator.mediaSession.metadata = cur
      ? new MediaMetadata({
          title: state?.is_live && state.stream_title ? state.stream_title : cur.title,
          artist: state?.is_live ? t("Radio web") : cur.artist,
          album: state?.is_live ? cur.uri : cur.album,
          artwork: cur.has_cover ? [{ src: api.coverUrl(cur.id), sizes: "512x512", type: "image/png" }] : [],
        })
      : null;
    navigator.mediaSession.setActionHandler("play", () => { void act(() => api.resume()); });
    navigator.mediaSession.setActionHandler("pause", () => { void act(() => api.pause()); });
    navigator.mediaSession.setActionHandler("previoustrack", () => { void act(() => api.prev()); });
    navigator.mediaSession.setActionHandler("nexttrack", () => { void act(() => api.next()); });
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [state?.current?.id, state?.current?.title, state?.status, state?.is_live, state?.stream_title]);

  // Radio contextuelle d'un artiste (#78) — émise par les lignes de pistes.
  useEffect(() => {
    const h = (e: Event) => {
      const artist = (e as CustomEvent<string>).detail;
      if (!artist) return;
      act(async () => {
        const r = await api.playArtistRadio(artist);
        say(t("Radio « {tag} » lancée pour « {artist} » ({count} radios)", { tag: r.tag, artist: r.artist, count: r.count }));
      });
    };
    window.addEventListener("auris:artist-radio", h);
    return () => window.removeEventListener("auris:artist-radio", h);
  }, [act, say]);

  const updateSettings = async (patch: (s: Settings) => Settings, okMsg?: string) => {
    if (!settings) return;
    const next = patch(settings);
    try {
      const back = await api.updateSettings(next);
      setSettings(back);
      if (okMsg) say(okMsg);
    } catch (e) { say(e instanceof Error ? e.message : String(e)); }
  };

  const playTrack = (t: Track) => act(() => api.playTrack(t.id));
  const playAll = (tracks: Track[]) => act(() => api.playQueue(tracks, 0));
  const playUri = (u: string) => act(() => api.play(u));

  /// Bascule un favori (piste/album/artiste) et resynchronise l'état local.
  const toggleFav = useCallback((kind: FavKind, id: string) => {
    act(async () => {
      await api.toggleFavorite(kind, id);
      setFavs(await api.favorites());
    });
  }, [act]);
  /// Bascule une épingle (album/playlist/radio) — #70.
  const togglePin = useCallback((kind: PinKind, id: string) => {
    act(async () => {
      await api.togglePin(kind, id);
      setPins(await api.pins());
    });
  }, [act]);
  const openSource = (id: string) => { setBrowseSrc(id); setView("browse"); };
  // Ouvre un serveur UPnP découvert à la racine de son ContentDirectory — #11.
  const openUpnp = (location: string) => { setBrowseSrc(`upnp://${location}/0`); setView("browse"); };
  // Recherche unique dans la topbar, contextuelle (#99) : live sur Bibliothèque et
  // Réglages, sinon (Entrée) bascule sur Bibliothèque avec la requête.
  const globalSearch = () => {
    if (!searchQ.trim()) return;
    setSearchNonce((n) => n + 1);
    if (view !== "library" && view !== "settings") setView("library");
  };
  const onTopSearch = (v: string) => {
    setSearchQ(v);
    if (view === "library" || view === "settings") setSearchNonce((n) => n + 1);
  };
  const refreshDevices = useCallback(async () => {
    try { setDevices(await api.devices()); } catch { /* inchangé si erreur */ }
  }, []);

  // Mode affichage plein écran « de loin » (#42) : #display dans l'URL à l'ouverture, Échap pour sortir.
  const hashInit = useRef(false);
  useEffect(() => {
    const h = location.hash.replace(/^#\/?/, "");
    if (!hashInit.current) {
      hashInit.current = true;
      if (h === "display") setView("display");
      return;
    }
    if (view === "display" && h !== "display") history.replaceState(null, "", "#display");
    else if (view !== "display" && h === "display") history.replaceState(null, "", location.pathname + location.search);
  }, [view]);
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => { if (e.key === "Escape" && view === "display") setView("home"); };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [view]);

  if (view === "display") {
    return <DisplayView state={state} act={act} onExit={() => setView("home")} />;
  }

  return (
    <div className="app">
      <div className="main">
        <Sidebar view={view} setView={setView} sources={sources} browseSrc={browseSrc} openSource={openSource} system={system} stats={stats} upnpDevs={upnpDevs} upnpScanning={upnpScanning} onScanUpnp={() => scanUpnp(true)} onOpenUpnp={openUpnp} />
        <main className="content">
          <div className="topbar">
            <div className="topbar-title">{view === "home" ? t("Accueil") : view === "library" ? t("Bibliothèque") : view === "browse" ? t("Parcourir") : view === "radios" ? t("Radios") : view === "queue" ? t("File d'attente") : view === "playlists" ? t("Listes de lecture") : view === "favorites" ? t("Favoris") : view === "sources" ? t("Sources") : view === "multiroom" ? t("Multiroom") : view === "plugins" ? t("Plugins") : t("Réglages")}</div>
            <div className="topbar-search">
              <Icon name="search" size={18} />
              <input type="text" placeholder={view === "settings" ? t("Rechercher dans les réglages…") : view === "library" ? t("Rechercher un titre, un album, un artiste…") : t("Rechercher un titre, un artiste…")} value={searchQ}
                onChange={(e) => onTopSearch(e.target.value)} onKeyDown={(e) => e.key === "Enter" && globalSearch()} />
              <button className="icon-btn" title={t("Rechercher")} onClick={globalSearch}><Icon name="search" size={18} /></button>
            </div>
          </div>
          {view === "home" && (
            <Home state={state} sources={sources} favs={favs} onToggleFav={toggleFav} pins={pins} onTogglePin={togglePin} playTrack={playTrack} playAll={playAll} openSource={openSource}
              openArtist={(a) => { setPendingArtist(a); setPendingAlbum(null); setView("library"); }}
              openAlbum={(a) => { setPendingAlbum(a); setPendingArtist(null); setView("library"); }} />
          )}
          {view === "library" && (
            <Library playTrack={playTrack} playAll={playAll} playUri={playUri} openSource={openSource} say={say} refresh={refresh} searchNonce={searchNonce} searchQ={searchQ} onClearSearch={() => setSearchQ("")}
              favs={favs} onToggleFav={toggleFav} pins={pins} onTogglePin={togglePin}
              pendingArtist={pendingArtist} onPendingArtistDone={() => setPendingArtist(null)}
              pendingAlbum={pendingAlbum} onPendingAlbumDone={() => setPendingAlbum(null)} />
          )}
          {view === "favorites" && (
            <FavoritesView playTrack={playTrack} playAll={playAll} favs={favs} onToggleFav={toggleFav} pins={pins} onTogglePin={togglePin}
              openAlbum={(a) => { setPendingAlbum(a); setPendingArtist(null); setView("library"); }}
              openArtist={(a) => { setPendingArtist(a); setPendingAlbum(null); setView("library"); }} />
          )}
          {view === "browse" && <Browse sources={sources} src={browseSrc} upnpDevs={upnpDevs} playUri={playUri} act={act} say={say} />}
          {view === "radios" && <Radios state={state} act={act} say={say} pins={pins} onTogglePin={togglePin} />}
          {view === "queue" && <Queue state={state} act={act} say={say} playAll={playAll} />}
          {view === "playlists" && <Playlists act={act} say={say} playAll={playAll} playTrack={playTrack} pins={pins} onTogglePin={togglePin} />}
          {view === "sources" && <Sources sources={sources} refresh={refresh} say={say} />}
          {view === "multiroom" && <MultiroomView state={state} act={act} say={say} />}
          {view === "plugins" && <PluginsView act={act} say={say} />}
          {view === "settings" && <SettingsView settings={settings} updateSettings={updateSettings} say={say} refresh={refresh} packs={packs} searchQ={searchQ} />}
        </main>
        {npOpen && state?.current && (
          <NowPlayingPanel
            track={state.current}
            pos={state.position_secs}
            tab={npTab}
            setTab={setNpTab}
            onClose={() => setNpOpen(false)}
            isLive={state.is_live}
            streamTitle={state.stream_title}
          />
        )}
      </div>
      {dsdCarrier != null && dop && (
        <DopBanner carrier={dsdCarrier} dop={dop} deviceName={state?.audio_device ?? null} backend={state?.output_backend ?? null} say={say}
          onChanged={() => api.dopStatus().then(setDop).catch(() => {})} />
      )}
      <PlayerBar state={state} devices={devices} act={act} say={say} alarms={alarms} refreshDevices={refreshDevices} npOpen={npOpen} onToggleNp={() => setNpOpen((v) => !v)} onToggleDisplay={() => setView("display")} onOpenSignalPath={() => setSpOpen(true)} />
      {spOpen && <SignalPathModal onClose={() => setSpOpen(false)} onOpenSettings={() => { setSpOpen(false); setView("settings"); }} />}
      {toast && <div className="toast">{toast}</div>}
    </div>
  );
}

function Sidebar({ view, setView, sources, browseSrc, openSource, system, stats, upnpDevs, upnpScanning, onScanUpnp, onOpenUpnp }: {
  view: View; setView: (v: View) => void; sources: Source[];
  browseSrc: string | null; openSource: (id: string) => void; system: SystemInfo | null;
  stats: LibraryStats | null; upnpDevs: UpnpDevice[]; upnpScanning: boolean;
  onScanUpnp: () => void; onOpenUpnp: (location: string) => void;
}) {
  const nav: [View, string, string][] = [
    ["home", t("Accueil"), "home"],
    ["library", t("Bibliothèque"), "note"],
    ["browse", t("Parcourir"), "folder"],
    ["radios", t("Radios"), "radio"],
    ["queue", t("File d'attente"), "queue"],
    ["playlists", t("Listes de lecture"), "playlist"],
    ["favorites", t("Favoris"), "heart"],
    ["sources", t("Sources"), "server"],
    ["multiroom", t("Multiroom"), "group"],
    ["plugins", t("Plugins"), "puzzle"],
    ["settings", t("Réglages"), "gear"],
  ];
  return (
    <nav className="sidebar">
      <div className="logo"><img src="/favicon.svg" alt="" /> auris</div>
      {nav.map(([v, label, icon]) => (
        <div key={v} className={`nav-item ${view === v ? "active" : ""}`} onClick={() => setView(v)}>
          <Icon name={icon} size={22} /><span>{label}</span>
        </div>
      ))}
      <div className="sep" />
      <div className="sources-label">{t("Vos sources")}</div>
      <div className="sources-list">
        {sources.map((s) => (
          <div key={s.id} className={`src-item ${view === "browse" && browseSrc === s.id ? "active" : ""}`} onClick={() => openSource(s.id)}>
            <span className="src-name"><Icon name={kindIconName(s.kind)} size={18} /> {s.name}</span>
            <span className={`dot ${s.enabled ? "on" : ""}`} />
          </div>
        ))}
      </div>
      <div className="sep" />
      <div className="sources-label">
        {t("Serveurs UPnP")}
        <button className="icon-btn" title={t("Actualiser la découverte UPnP (SSDP)")} disabled={upnpScanning} onClick={onScanUpnp}>
          <Icon name="refresh" size={14} />
        </button>
      </div>
      <div className="sources-list">
        {upnpDevs.map((d) => (
          <div key={d.location} className={`src-item ${view === "browse" && browseSrc?.startsWith(`upnp://${d.location}`) ? "active" : ""}`}
            title={d.location} onClick={() => onOpenUpnp(d.location)}>
            <span className="src-name"><Icon name="server" size={18} /> {d.friendly_name}</span>
          </div>
        ))}
        {!upnpScanning && upnpDevs.length === 0 && (
          <div className="src-empty">{t("Aucun serveur UPnP détecté — bouton actualiser, ou ajoutez une URL UPnP dans Sources.")}</div>
        )}
        {upnpScanning && <div className="src-empty">{t("Découverte UPnP en cours…")}</div>}
      </div>
      {system && (
        <div className="sysinfo">
          v{system.version}
          {stats && stats.tracks > 0 && (
            <>
              <br />{cl(stats.artists, "artiste", "artistes")} · {cl(stats.albums, "album", "albums")} · {cl(stats.tracks, "titre", "titres")} · {fmtPlaytime(stats.playtime_secs)}
            </>
          )}
        </div>
      )}
    </nav>
  );
}

/// Durée longue façon Volumio collectionstats : « 1220:17:22 » (h:mm:ss).
function fmtPlaytime(secs: number): string {
  if (!isFinite(secs) || secs < 0) secs = 0;
  const h = Math.floor(secs / 3600);
  const m = Math.floor((secs % 3600) / 60);
  const s = Math.floor(secs % 60);
  return `${h}:${m.toString().padStart(2, "0")}:${s.toString().padStart(2, "0")}`;
}

/// Une rangée horizontale de grandes cartes (façon Nova/Spotify).
function Rail({ title, children }: { title: string; children: React.ReactNode }) {
  return (
    <section className="rail-sec">
      <h2>{title}</h2>
      <div className="rail">{children}</div>
    </section>
  );
}

/// Bouton cœur de favori (#68) : `on` = rempli. Le clic ne propage pas (cartes/ lignes cliquables).
function FavHeart({ on, onToggle, title }: { on: boolean; onToggle: () => void; title?: string }) {
  return (
    <button
      className={`fav-heart${on ? " on" : ""}`}
      title={title ?? (on ? t("Retirer des favoris") : t("Ajouter aux favoris"))}
      onClick={(e) => { e.stopPropagation(); e.preventDefault(); onToggle(); }}
    >
      <Icon name="heart" size={16} filled={on} />
    </button>
  );
}

/// Bouton épingle (#70) : `on` = épinglé. Le clic ne propage pas.
function PinButton({ on, onToggle }: { on: boolean; onToggle: () => void }) {
  return (
    <button
      className={`fav-heart pin-btn${on ? " on" : ""}`}
      title={on ? t("Retirer des épingles") : t("Épingler")}
      onClick={(e) => { e.stopPropagation(); e.preventDefault(); onToggle(); }}
    >
      <Icon name="pin" size={14} filled={on} />
    </button>
  );
}

function FacetCard({ coverUrl, icon, title, sub, round, fav, onToggleFav, pinned, onTogglePin, onClick }: { coverUrl: string | null; icon: string; title: string; sub: string; round?: boolean; fav?: boolean; onToggleFav?: () => void; pinned?: boolean; onTogglePin?: () => void; onClick: () => void }) {
  return (
    <div className={`card${round ? " round" : ""}`} onClick={onClick}>
      <div className="cover">{coverUrl ? <img src={coverUrl} alt="" loading="lazy" /> : <Icon name={icon} size={44} />}
        {onTogglePin && <PinButton on={!!pinned} onToggle={onTogglePin} />}
        {onToggleFav && <FavHeart on={!!fav} onToggle={onToggleFav} />}
      </div>
      <div className="t">{title}</div>
      <div className="s">{sub}</div>
    </div>
  );
}

type Facet = { kind: "genre"; name: string } | { kind: "decade"; decade: number };

type LibProps = {
  favs: Favorites;
  onToggleFav: (kind: FavKind, id: string) => void;
  pins: Pins;
  onTogglePin: (kind: PinKind, id: string) => void;
};

function Home({ state, sources, favs, onToggleFav, pins, onTogglePin, playTrack, playAll, openSource, openArtist, openAlbum }: { state: PlayerState | null; sources: Source[]; playTrack: (t: Track) => void; playAll: (t: Track[]) => void; openSource: (id: string) => void; openArtist: (a: { id: string; name: string }) => void; openAlbum: (a: { id: string; title: string; artist: string }) => void } & LibProps) {
  const [albums, setAlbums] = useState<Album[]>([]);
  const [tracks, setTracks] = useState<Track[] | null>(null);
  const [artists, setArtists] = useState<Artist[]>([]);
  const [facets, setFacets] = useState<Facets | null>(null);
  const [history, setHistory] = useState<HistoryTrack[]>([]);
  const [facet, setFacet] = useState<Facet | null>(null);
  useEffect(() => {
    api.albums().then(setAlbums).catch(() => {});
    api.tracksPage({ limit: 20 }).then((r) => setTracks(r.items)).catch(() => {});
    api.artists().then(setArtists).catch(() => {});
    api.facets().then(setFacets).catch(() => {});
    api.history(14).then(setHistory).catch(() => {});
  }, []);

  if (facet) {
    return (
      <FacetView
        facet={facet}
        playTrack={playTrack}
        playAll={playAll}
        favs={favs}
        onToggleFav={onToggleFav}
        pins={pins}
        onTogglePin={onTogglePin}
        onBack={() => setFacet(null)}
        onOpenAlbum={openAlbum}
      />
    );
  }

  const favAlbums = albums.filter((a) => favs.albums.includes(a.id));
  const favArtists = artists.filter((a) => favs.artists.includes(a.id));
  const pinnedAlbums = pins.albums.map((id) => albums.find((a) => a.id === id)).filter((a): a is Album => !!a);

  return (
    <div>
      {pinnedAlbums.length > 0 && (
        <Rail title={t("Épinglés")}>
          {pinnedAlbums.slice(0, 14).map((a) => (
            <FacetCard key={a.id} coverUrl={a.cover_track_id ? api.coverUrl(a.cover_track_id) : null} icon="note" title={a.title}
              sub={`${a.artist}${a.year ? ` · ${a.year}` : ""}`}
              fav={favs.albums.includes(a.id)} onToggleFav={() => onToggleFav("album", a.id)}
              pinned onTogglePin={() => onTogglePin("album", a.id)}
              onClick={() => openAlbum({ id: a.id, title: a.title, artist: a.artist })} />
          ))}
        </Rail>
      )}
      {state?.current && (
        <Rail title={t("En lecture")}>
          <div className="card" onClick={() => playTrack(state.current!)}>
            <div className="cover">{state.current.has_cover ? <img src={api.coverUrl(state.current.id)} alt="" /> : <Icon name="note" size={44} />}
              <FavHeart on={favs.tracks.includes(state.current.id)} onToggle={() => onToggleFav("track", state.current!.id)} />
            </div>
            <div className="t">{state.current.title}</div>
            <div className="s">{state.current.artist} — {state.current.album}</div>
          </div>
        </Rail>
      )}
      {history.length > 0 && (
        <Rail title={t("Écoutés récemment")}>
          {history.map((tr) => (
            <FacetCard key={`${tr.id}-${tr.last_played}`} coverUrl={tr.has_cover ? api.coverUrl(tr.id) : null} icon="note" title={tr.title}
              sub={`${tr.artist} — ${tr.album}`}
              fav={favs.tracks.includes(tr.id)}
              onToggleFav={() => onToggleFav("track", tr.id)}
              onClick={() => playTrack(tr)} />
          ))}
        </Rail>
      )}
      {favAlbums.length > 0 && (
        <Rail title={t("Favoris")}>
          {favAlbums.slice(0, 14).map((a) => (
            <FacetCard key={a.id} coverUrl={a.cover_track_id ? api.coverUrl(a.cover_track_id) : null} icon="note" title={a.title}
              sub={`${a.artist}${a.year ? ` · ${a.year}` : ""}`}
              fav onToggleFav={() => onToggleFav("album", a.id)}
              pinned={pins.albums.includes(a.id)} onTogglePin={() => onTogglePin("album", a.id)}
              onClick={() => openAlbum({ id: a.id, title: a.title, artist: a.artist })} />
          ))}
        </Rail>
      )}
      {facets && facets.genres.length > 0 && (
        <Rail title={t("Genres")}>
          {facets.genres.slice(0, 14).map((g) => (
            <FacetCard key={g.name} coverUrl={g.cover_url} icon="note" title={g.name}
              sub={cl(g.track_count, "titre", "titres")}
              onClick={() => setFacet({ kind: "genre", name: g.name })} />
          ))}
        </Rail>
      )}
      {facets && facets.decades.length > 0 && (
        <Rail title={t("Décennies")}>
          {facets.decades.slice(0, 10).map((d) => (
            <FacetCard key={d.decade} coverUrl={d.cover_url} icon="alarm" title={t("Années {y}", { y: d.decade })}
              sub={cl(d.track_count, "titre", "titres")}
              onClick={() => setFacet({ kind: "decade", decade: d.decade })} />
          ))}
        </Rail>
      )}
      {artists.length > 0 && (
        <Rail title={t("Artistes")}>
          {artists.slice(0, 14).map((a) => (
            <FacetCard key={a.id} coverUrl={null} icon="group" title={a.name} round
              sub={cl(a.album_count, "album", "albums")}
              fav={favs.artists.includes(a.id)}
              onToggleFav={() => onToggleFav("artist", a.id)}
              onClick={() => openArtist({ id: a.id, name: a.name })} />
          ))}
        </Rail>
      )}
      <Rail title={t("Albums récents")}>
        {albums.slice(0, 14).map((a) => (
          <FacetCard key={a.id} coverUrl={a.cover_track_id ? api.coverUrl(a.cover_track_id) : null} icon="note" title={a.title}
            sub={`${a.artist}${a.year ? ` · ${a.year}` : ""}`}
            fav={favs.albums.includes(a.id)}
            onToggleFav={() => onToggleFav("album", a.id)}
            pinned={pins.albums.includes(a.id)}
            onTogglePin={() => onTogglePin("album", a.id)}
            onClick={() => openAlbum({ id: a.id, title: a.title, artist: a.artist })} />
        ))}
        {albums.length === 0 && <div className="empty">{t("Scannez une source pour remplir la bibliothèque.")}</div>}
      </Rail>
      {favArtists.length > 0 && (
        <Rail title={t("Artistes favoris")}>
          {favArtists.slice(0, 14).map((a) => (
            <FacetCard key={a.id} coverUrl={null} icon="group" title={a.name} round
              sub={cl(a.album_count, "album", "albums")}
              fav onToggleFav={() => onToggleFav("artist", a.id)}
              onClick={() => openArtist({ id: a.id, name: a.name })} />
          ))}
        </Rail>
      )}
      <Rail title={t("Vos sources")}>
        {sources.map((s) => (
          <FacetCard key={s.id} coverUrl={null} icon={kindIconName(s.kind)} title={s.name}
            sub={`${kindLabel(s.kind)}${s.enabled ? "" : ` · ${t("désactivée")}`}`}
            onClick={() => openSource(s.id)} />
        ))}
        {sources.length === 0 && <div className="empty">{t("Aucune source — ajoutez-en une dans « Sources & réglages ».")}</div>}
      </Rail>
      <h2>{t("Titres récents")}</h2>
      <TrackTable tracks={tracks ?? []} loading={tracks === null} current={state?.current?.id} favs={favs} onToggleFav={onToggleFav} playTrack={playTrack} playAll={playAll} />
    </div>
  );
}

/// Vue d'une facette (genre ou décennie) : en-tête + albums concernés + titres.
function FacetView({ facet, playTrack, playAll, favs, onToggleFav, pins, onTogglePin, onBack, onOpenAlbum }: { facet: Facet; playTrack: (t: Track) => void; playAll: (t: Track[]) => void; onBack: () => void; onOpenAlbum: (a: { id: string; title: string; artist: string }) => void } & LibProps) {
  const [tracks, setTracks] = useState<Track[] | null>(null);
  const [albums, setAlbums] = useState<Album[]>([]);
  const title = facet.kind === "genre" ? facet.name : t("Années {y}", { y: facet.decade });
  useEffect(() => {
    setTracks(null);
    api.tracks(facet.kind === "genre" ? facet.name : undefined, facet.kind === "decade" ? facet.decade : undefined)
      .then(setTracks).catch(() => setTracks([]));
    api.albums().then(setAlbums).catch(() => {});
  }, [facet]);
  const keys = new Set((tracks ?? []).map((t) => `${t.album_artist}|${t.album}`.toLowerCase()));
  const facetAlbums = albums.filter((a) => keys.has(`${a.artist}|${a.title}`.toLowerCase()));
  const coverTrack = (tracks ?? []).find((t) => t.has_cover);
  return (
    <div>
      <div className="toolbar">
        <button className="btn ghost sm" onClick={onBack}><Icon name="back" size={16} /> {t("Retour")}</button>
        <button className="btn sm" disabled={!tracks?.length} onClick={() => tracks && playAll(tracks)}><Icon name="play" size={14} filled /> {t("Tout lire")}</button>
      </div>
      <div className="artist-head">
        <div className="cover big">{coverTrack ? <img src={api.coverUrl(coverTrack.id)} alt="" /> : <Icon name="note" size={44} />}</div>
        <div>
          <div className="s" style={{ textTransform: "uppercase", letterSpacing: 1 }}>{facet.kind === "genre" ? t("Genre") : t("Décennie")}</div>
          <h1 style={{ margin: "4px 0" }}>{title}</h1>
          <div className="s">{cl(facetAlbums.length, "album", "albums")} · {cl(tracks?.length ?? 0, "titre", "titres")}</div>
        </div>
      </div>
      {facetAlbums.length > 0 && (
        <>
          <h2>{t("Albums")}</h2>
          <div className="grid">
            {facetAlbums.map((a) => (
              <div key={a.id} className="card" onClick={() => onOpenAlbum({ id: a.id, title: a.title, artist: a.artist })}>
                <div className="cover">{a.cover_track_id ? <img src={api.coverUrl(a.cover_track_id)} alt="" loading="lazy" /> : <Icon name="note" size={44} />}
                  <PinButton on={pins.albums.includes(a.id)} onToggle={() => onTogglePin("album", a.id)} />
                  <FavHeart on={favs.albums.includes(a.id)} onToggle={() => onToggleFav("album", a.id)} />
                </div>
                <div className="t">{a.title}</div>
                <div className="s">{a.artist}{a.year ? ` · ${a.year}` : ""} · {cl(a.track_count, "titre", "titres")}</div>
              </div>
            ))}
          </div>
        </>
      )}
      <h2>{t("Titres")}</h2>
      {tracks === null ? <div className="empty">{t("Chargement…")}</div> : <TrackTable tracks={tracks} playTrack={playTrack} playAll={playAll} favs={favs} onToggleFav={onToggleFav} />}
    </div>
  );
}

function Library({ playTrack, playAll, playUri, openSource, say, refresh, searchNonce, searchQ, onClearSearch, pendingArtist, onPendingArtistDone, pendingAlbum, onPendingAlbumDone, favs, onToggleFav, pins, onTogglePin }: { playTrack: (t: Track) => void; playAll: (t: Track[]) => void; playUri: (u: string) => void; openSource: (id: string) => void; say: (m: string) => void; refresh: () => Promise<void>; searchNonce: number; searchQ: string; onClearSearch: () => void; pendingArtist: { id: string; name: string } | null; onPendingArtistDone: () => void; pendingAlbum: { id: string; title: string; artist: string } | null; onPendingAlbumDone: () => void } & LibProps) {
  const [tab, setTab] = useState<"tracks" | "albums" | "artists">("tracks");
  // Onglet Titres paginé (#89) : on ne charge jamais les 16 k pistes d'un coup.
  const [tracks, setTracks] = useState<Track[] | null>(null);
  const [tracksTotal, setTracksTotal] = useState(0);
  const [tracksLoading, setTracksLoading] = useState(false);
  const [albums, setAlbums] = useState<Album[] | null>(null);
  const [artists, setArtists] = useState<Artist[] | null>(null);
  const [q, setQ] = useState("");
  const [searching, setSearching] = useState(false);
  const [results, setResults] = useState<SearchResults | null>(null);
  const [kind, setKind] = useState<"all" | "tracks" | "albums" | "artists" | "playlists" | "radios" | "sources">("all");
  const [srcFilter, setSrcFilter] = useState("");
  const [sources, setSources] = useState<Source[]>([]);
  const [plDetail, setPlDetail] = useState<PlaylistDetail | null>(null);
  const [scanning, setScanning] = useState(false);
  const [albumDetail, setAlbumDetail] = useState<{ id: string; title: string; artist: string } | null>(null);
  const [artistDetail, setArtistDetail] = useState<{ id: string; name: string } | null>(null);
  const TRACKS_PAGE = 200;
  const loadTracksPage = (offset: number) => {
    setTracksLoading(true);
    api.tracksPage({ limit: TRACKS_PAGE, offset })
      .then((r) => {
        setTracks((prev) => (offset === 0 ? r.items : [...(prev ?? []), ...r.items]));
        setTracksTotal(r.total);
      })
      .catch(() => {})
      .finally(() => setTracksLoading(false));
  };
  // Ouverture demandée depuis l'accueil (carte artiste/album) : on ouvre la vue correspondante.
  useEffect(() => {
    if (pendingArtist) { setArtistDetail(pendingArtist); setAlbumDetail(null); onPendingArtistDone(); }
  }, [pendingArtist, onPendingArtistDone]);
  useEffect(() => {
    if (pendingAlbum) { setAlbumDetail(pendingAlbum); setArtistDetail(null); onPendingAlbumDone(); }
  }, [pendingAlbum, onPendingAlbumDone]);
  useEffect(() => {
    loadTracksPage(0);
    api.albums().then(setAlbums).catch(() => {});
    api.artists().then(setArtists).catch(() => {});
    api.sources().then(setSources).catch(() => {});
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);
  const runSearch = (query: string, src: string) => {
    if (!query.trim()) {
      setSearching(false);
      setResults(null);
      loadTracksPage(0);
      return;
    }
    setSearching(true);
    api.search(query, src || undefined).then(setResults).catch(() => {});
  };
  // La recherche vient de la topbar unique (#99) : on débounce à 250 ms car chaque
  // frappe en Bibliothèque bump le nonce.
  useEffect(() => {
    if (searchNonce > 0) {
      const h = setTimeout(() => {
        setQ(searchQ);
        setKind("all");
        runSearch(searchQ, srcFilter);
      }, 250);
      return () => clearTimeout(h);
    }
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [searchNonce]);
  // Recherche instantanée (debounce 250 ms) pendant que le mode recherche est actif.
  useEffect(() => {
    if (!searching || !q.trim()) return;
    const h = setTimeout(() => runSearch(q, srcFilter), 250);
    return () => clearTimeout(h);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [q, srcFilter, searching]);
  const clearSearch = () => {
    setSearching(false);
    setResults(null);
    setQ("");
    onClearSearch();
    loadTracksPage(0);
  };
  const scan = async () => {
    setScanning(true);
    try {
      const id = (await api.sources()).find((s) => s.enabled)?.id;
      if (!id) { say("Aucune source active"); setScanning(false); return; }
      await api.scan(id);
      say(`Scan de ${id} lancé`);
      setTimeout(() => { refresh(); setScanning(false); }, 1500);
    } catch (e) { say(e instanceof Error ? e.message : String(e)); setScanning(false); }
  };
  if (albumDetail) {
    return <AlbumView id={albumDetail.id} title={albumDetail.title} artist={albumDetail.artist} playAll={playAll} playTrack={playTrack} say={say} favs={favs} onToggleFav={onToggleFav} pins={pins} onTogglePin={onTogglePin} onBack={() => setAlbumDetail(null)} />;
  }
  if (artistDetail) {
    return <ArtistView id={artistDetail.id} name={artistDetail.name} playAll={playAll} playTrack={playTrack} favs={favs} onToggleFav={onToggleFav} pins={pins} onTogglePin={onTogglePin} onBack={() => setArtistDetail(null)} onOpenAlbum={(a) => setAlbumDetail(a)} />;
  }
  if (plDetail) {
    return (
      <div>
        <div className="toolbar">
          <button className="btn ghost sm" onClick={() => setPlDetail(null)}><Icon name="back" size={16} /> {t("Retour")}</button>
          <div className="spacer" />
          <button className="btn sm" disabled={!plDetail.tracks.length} onClick={() => playAll(plDetail.tracks)}><Icon name="play" size={14} filled /> {t("Tout lire")}</button>
        </div>
        <h1>{plDetail.playlist.name}</h1>
        <TrackTable tracks={plDetail.tracks} playTrack={playTrack} playAll={playAll} favs={favs} onToggleFav={onToggleFav} />
      </div>
    );
  }
  return (
    <div>
      {searching ? (
        results ? (
        <div>
          <div className="toolbar" style={{ marginBottom: 8 }}>
            <div className="spacer" />
            <button className="btn ghost sm" onClick={clearSearch}>{t("Effacer la recherche")}</button>
          </div>
          <div className="chips" style={{ marginTop: 0, marginBottom: 4 }}>
            {([["all", t("Tous")], ["tracks", `${t("Titres")} (${results.tracks.total})`], ["albums", `${t("Albums")} (${results.albums.total})`], ["artists", `${t("Artistes")} (${results.artists.total})`], ["playlists", `${t("Listes")} (${results.playlists.total})`], ["radios", `${t("Radios")} (${results.radios?.total ?? 0})`], ["sources", `${t("Sources")} (${results.sources?.total ?? 0})`]] as const).map(([k, label]) => (
              <button key={k} className={`chip chip-btn ${kind === k ? "active" : ""}`} onClick={() => setKind(k)}>{label}</button>
            ))}
          </div>
          {sources.some((s) => s.enabled) && (
            <div className="chips" style={{ marginTop: 0, marginBottom: 8 }}>
              <button className={`chip chip-btn ${srcFilter === "" ? "active" : ""}`} onClick={() => setSrcFilter("")}>{t("Toutes les sources")}</button>
              {sources.filter((s) => s.enabled).map((s) => (
                <button key={s.id} className={`chip chip-btn ${srcFilter === s.id ? "active" : ""}`} onClick={() => setSrcFilter(s.id)}>{s.name}</button>
              ))}
            </div>
          )}
          {results.tracks.total + results.albums.total + results.artists.total + results.playlists.total + (results.radios?.total ?? 0) + (results.sources?.total ?? 0) === 0 && (
            <div className="empty">{t("Aucun résultat pour « {q} »", { q })}</div>
          )}
          {(kind === "all" || kind === "artists") && results.artists.items.length > 0 && (
            <>
              <h2>{t("Artistes")} ({results.artists.total})</h2>
              <table className="tracks"><tbody>
                {results.artists.items.map((a) => <tr key={a.id} className="link" onClick={() => setArtistDetail({ id: a.id, name: a.name })}><td>{a.name}</td><td className="meta"><FavHeart on={favs.artists.includes(a.id)} onToggle={() => onToggleFav("artist", a.id)} /> {cl(a.album_count, "album", "albums")}</td></tr>)}
              </tbody></table>
            </>
          )}
          {(kind === "all" || kind === "albums") && results.albums.items.length > 0 && (
            <>
              <h2>{t("Albums")} ({results.albums.total})</h2>
              <div className="grid">
                {results.albums.items.map((a) => (
                  <div key={a.id} className="card" onClick={() => setAlbumDetail({ id: a.id, title: a.title, artist: a.artist })}>
                    <div className="cover">{a.cover_track_id ? <img src={api.coverUrl(a.cover_track_id)} alt="" loading="lazy" /> : <Icon name="note" size={44} />}
                      <PinButton on={pins.albums.includes(a.id)} onToggle={() => onTogglePin("album", a.id)} />
                      <FavHeart on={favs.albums.includes(a.id)} onToggle={() => onToggleFav("album", a.id)} />
                    </div>
                    <div className="t">{a.title}</div>
                    <div className="s">{a.artist} · {cl(a.track_count, "titre", "titres")} · {fmtTime(a.duration_secs)}</div>
                  </div>
                ))}
              </div>
            </>
          )}
          {(kind === "all" || kind === "playlists") && results.playlists.items.length > 0 && (
            <>
              <h2>{t("Listes de lecture")} ({results.playlists.total})</h2>
              <table className="tracks"><tbody>
                {results.playlists.items.map((p) => (
                  <tr key={p.id} className="link" onClick={() => api.playlist(p.id).then(setPlDetail).catch(() => {})}>
                    <td><Icon name="playlist" size={16} /> {p.name}</td>
                    <td className="meta"><PinButton on={pins.playlists.includes(p.id)} onToggle={() => onTogglePin("playlist", p.id)} /> {cl(p.track_ids.length, "titre", "titres")}</td>
                  </tr>
                ))}
              </tbody></table>
            </>
          )}
          {(kind === "all" || kind === "radios") && (results.radios?.items.length ?? 0) > 0 && results.radios && (
            <>
              <h2>{t("Radios")} ({results.radios.total})</h2>
              <table className="tracks"><tbody>
                {results.radios.items.map((r) => (
                  <tr key={r.id} className="link" onClick={() => api.playRadio(r.id).catch(() => {})}>
                    <td><Icon name="radio" size={16} /> {r.name}</td>
                    <td className="meta">{r.origin === "favorite" ? t("Favori") : r.origin === "builtin" ? t("Intégrée") : t("Source")}</td>
                  </tr>
                ))}
              </tbody></table>
            </>
          )}
          {(kind === "all" || kind === "sources") && (results.sources?.items.length ?? 0) > 0 && results.sources && (
            <>
              <h2>{t("Sources externes")} ({results.sources.total})</h2>
              <table className="tracks"><tbody>
                {results.sources.items.map((e) => (
                  <tr key={e.uri} className="link" onClick={() => e.is_audio ? playUri(e.uri) : openSource(e.uri.replace(/^src:\/\/([^/]+)\/.*$/, "$1"))}>
                    <td><Icon name={e.is_dir ? "folder" : "note"} size={16} /> {e.name}</td>
                    <td className="meta">{e.is_dir ? t("Dossier — ouvrir dans Parcourir") : e.format?.kind === "pcm" ? `${e.format.bits} bits / ${Math.round((e.format.sample_rate ?? 0) / 1000)} kHz` : t("Piste")}</td>
                  </tr>
                ))}
              </tbody></table>
            </>
          )}
          {(kind === "all" || kind === "tracks") && results.tracks.items.length > 0 && (
            <>
              <h2>{t("Titres")} ({results.tracks.total})</h2>
              <TrackTable tracks={results.tracks.items} playTrack={playTrack} playAll={playAll} favs={favs} onToggleFav={onToggleFav} />
            </>
          )}
        </div>
        ) : (
          <div className="empty">{t("Recherche…")}</div>
        )
      ) : (
        <>
      <div className="toolbar">
        <div className="seg">
          {([["tracks", "Titres"], ["albums", "Albums"], ["artists", "Artistes"]] as const).map(([t, label]) => (
            <button key={t} className={`seg-btn ${tab === t ? "active" : ""}`} onClick={() => setTab(t)}>{label}</button>
          ))}
        </div>
        <div className="spacer" />
        <button className="btn sm" disabled={scanning} onClick={scan}>{scanning ? t("Scan…") : t("Scanner la source")}</button>
      </div>
      {tab === "tracks" && (
        <>
          <TrackTable tracks={tracks ?? []} loading={tracks === null} playTrack={playTrack} playAll={playAll} favs={favs} onToggleFav={onToggleFav} />
          {tracks !== null && tracks.length < tracksTotal && (
            <div className="see-more">
              <button className="btn ghost" disabled={tracksLoading} onClick={() => loadTracksPage(tracks.length)}>
                {tracksLoading ? t("Chargement…") : t("Voir plus ({n} restantes)", { n: tracksTotal - tracks.length })}
              </button>
            </div>
          )}
          {tracks !== null && tracks.length >= tracksTotal && tracksTotal > TRACKS_PAGE && (
            <div className="empty" style={{ padding: "12px 0" }}>{t("{n} titres au total", { n: tracksTotal })}</div>
          )}
        </>
      )}
      {tab === "albums" && (albums === null ? <div className="empty">{t("Chargement…")}</div> : (
        <div className="grid">
          {albums.map((a) => (
            <div key={a.id} className="card" onClick={() => setAlbumDetail({ id: a.id, title: a.title, artist: a.artist })}>
              <div className="cover">{a.cover_track_id ? <img src={api.coverUrl(a.cover_track_id)} alt="" loading="lazy" /> : <Icon name="note" size={44} />}
                <PinButton on={pins.albums.includes(a.id)} onToggle={() => onTogglePin("album", a.id)} />
                <FavHeart on={favs.albums.includes(a.id)} onToggle={() => onToggleFav("album", a.id)} />
              </div>
              <div className="t">{a.title}</div>
              <div className="s">{a.artist} · {a.track_count} titres · {fmtTime(a.duration_secs)}</div>
            </div>
          ))}
        </div>
      ))}
      {tab === "artists" && (artists === null ? <div className="empty">{t("Chargement…")}</div> : (
        <div className="artist-grid">
          {artists.map((a) => (
            <div key={a.id} className="artist-card" onClick={() => setArtistDetail({ id: a.id, name: a.name })}>
              <div className="artist-cover">{a.cover_track_id ? <img src={api.coverUrl(a.cover_track_id)} alt="" loading="lazy" /> : <Icon name="user" size={52} />}</div>
              <div className="artist-name" title={a.name}>{a.name}</div>
              <div className="artist-sub"><FavHeart on={favs.artists.includes(a.id)} onToggle={() => onToggleFav("artist", a.id)} /> {cl(a.album_count, "album", "albums")}</div>
            </div>
          ))}
        </div>
      ))}
        </>
      )}
    </div>
  );
}


function ArtistView({ id, name, playAll, playTrack, favs, onToggleFav, pins, onTogglePin, onBack, onOpenAlbum }: { id: string; name: string; playAll: (t: Track[]) => void; playTrack: (t: Track) => void; onBack: () => void; onOpenAlbum: (a: { id: string; title: string; artist: string }) => void } & LibProps) {
  const [detail, setDetail] = useState<ArtistDetail | null>(null);
  useEffect(() => { api.artist(id).then(setDetail).catch(() => {}); }, [id]);
  const tracks = detail?.tracks ?? [];
  const albums = detail?.albums ?? [];
  const genres = detail?.genres ?? [];
  const years = detail?.year_min != null && detail?.year_max != null
    ? (detail.year_min === detail.year_max ? `${detail.year_min}` : `${detail.year_min}–${detail.year_max}`)
    : null;
  return (
    <div>
      <div className="toolbar">
        <button className="btn ghost sm" onClick={onBack}><Icon name="back" size={16} /> {t("Retour")}</button>
        <button className="btn sm" disabled={!tracks.length} onClick={() => playAll(tracks)}><Icon name="play" size={14} filled /> {t("Tout lire")}</button>
      </div>
      <div className="artist-head">
        <div className="cover big">{detail?.cover_url ? <img src={detail.cover_url} alt="" /> : <Icon name="user" size={44} />}
          <FavHeart on={favs.artists.includes(id)} onToggle={() => onToggleFav("artist", id)} />
        </div>
        <div>
          <div className="s" style={{ textTransform: "uppercase", letterSpacing: 1 }}>{t("Artiste")}</div>
          <h1 style={{ margin: "4px 0" }}>{name}</h1>
          <div className="s">
            {cl(albums.length, "album", "albums")} · {cl(tracks.length, "titre", "titres")}
            {years ? ` · ${years}` : ""}
            {detail && detail.duration_secs > 0 ? ` · ${fmtTime(detail.duration_secs)}` : ""}
          </div>
          {genres.length > 0 && (
            <div className="chips">{genres.map((g) => <span key={g} className="chip">{g}</span>)}</div>
          )}
        </div>
      </div>
      {detail?.bio && <p className="bio">{detail.bio}</p>}
      {albums.length > 0 && (
        <>
          <h2>{t("Albums")}</h2>
          <div className="grid">
            {albums.map((a) => (
              <div key={a.id} className="card" onClick={() => onOpenAlbum({ id: a.id, title: a.title, artist: a.artist })}>
                <div className="cover">{a.cover_track_id ? <img src={api.coverUrl(a.cover_track_id)} alt="" loading="lazy" /> : <Icon name="note" size={44} />}
                  <PinButton on={pins.albums.includes(a.id)} onToggle={() => onTogglePin("album", a.id)} />
                  <FavHeart on={favs.albums.includes(a.id)} onToggle={() => onToggleFav("album", a.id)} />
                </div>
                <div className="t">{a.title}</div>
                <div className="s">{a.artist}{a.year ? ` · ${a.year}` : ""} · {cl(a.track_count, "titre", "titres")}</div>
              </div>
            ))}
          </div>
        </>
      )}
      <h2>{t("Titres")}</h2>
      <TrackTable tracks={tracks} playTrack={playTrack} playAll={playAll} favs={favs} onToggleFav={onToggleFav} />
    </div>
  );
}

function AlbumView({ id, title, artist, playAll, playTrack, say, favs, onToggleFav, pins, onTogglePin, onBack }: { id: string; title: string; artist: string; playAll: (t: Track[]) => void; playTrack: (t: Track) => void; say: (m: string) => void; onBack: () => void } & LibProps) {
  const [tracks, setTracks] = useState<Track[]>([]);
  const [cover, setCover] = useState<string | null>(null);
  const [album, setAlbum] = useState<Album | null>(null);
  useEffect(() => {
    fetch(`/api/library/albums/${encodeURIComponent(id)}`).then((r) => r.json()).then((j) => {
      setTracks(j.tracks ?? []);
      setCover(j.cover_url ?? null);
      setAlbum(j.album ?? null);
    }).catch(() => {});
  }, [id]);
  return (
    <div>
      <div className="toolbar">
        <button className="btn ghost sm" onClick={onBack}><Icon name="back" size={16} /> Retour</button>
        <button className="btn sm" onClick={() => tracks.length && playAll(tracks)}><Icon name="play" size={14} filled /> Tout lire</button>
        <button className="btn ghost sm" onClick={() => { window.location.href = api.albumExportUrl(id); }}><Icon name="download" size={16} /> Export ZIP</button>
        <button className="btn ghost sm" onClick={async () => {
          try {
            const sh = await api.createShare("album", id);
            const url = api.shareUrl(sh.token);
            await navigator.clipboard?.writeText(url).catch(() => {});
            say(`Lien de partage copié : ${url}`);
          } catch (e) { say(e instanceof Error ? e.message : String(e)); }
        }}><Icon name="share" size={16} /> Partager</button>
      </div>
      <h1>{title} <PinButton on={pins.albums.includes(id)} onToggle={() => onTogglePin("album", id)} /> <FavHeart on={favs.albums.includes(id)} onToggle={() => onToggleFav("album", id)} /></h1>
      <h2 style={{ marginTop: 0 }}>{artist}</h2>
      {album && [album.year, album.label, album.publisher, album.copyright].some((v) => v != null && v !== "") && (
        <div className="s" style={{ marginTop: -10, marginBottom: 10 }}>
          {[album.year, album.label, album.publisher, album.copyright].filter((v) => v != null && v !== "").join(" · ")}
        </div>
      )}
      <TrackTable tracks={tracks} playTrack={playTrack} playAll={playAll} favs={favs} onToggleFav={onToggleFav} albumCover={cover} />
      {tracks.some((t) => t.composer || t.producer || t.isrc) && (
        <>
          <h2>{t("Crédits")}</h2>
          <table className="tracks">
            <thead><tr><th>#</th><th>{t("Titre")}</th><th>{t("Compositeur")}</th><th>{t("Producteur")}</th><th>ISRC</th></tr></thead>
            <tbody>
              {tracks.filter((t) => t.composer || t.producer || t.isrc).map((t) => (
                <tr key={t.id} className="link" onClick={() => playTrack(t)}>
                  <td className="meta">{t.track_number ?? ""}</td>
                  <td>{t.title}</td>
                  <td>{t.composer ?? ""}</td>
                  <td>{t.producer ?? ""}</td>
                  <td className="meta">{t.isrc ?? ""}</td>
                </tr>
              ))}
            </tbody>
          </table>
        </>
      )}
    </div>
  );
}

/// Vue Favoris (#68) : albums, artistes et titres aimés, résolus côté serveur.
function FavoritesView({ playTrack, playAll, favs, onToggleFav, pins, onTogglePin, openAlbum, openArtist }: { playTrack: (t: Track) => void; playAll: (t: Track[]) => void; openAlbum: (a: { id: string; title: string; artist: string }) => void; openArtist: (a: { id: string; name: string }) => void } & LibProps) {
  const [data, setData] = useState<FavoritesResolved | null>(null);
  useEffect(() => { api.favoritesResolved().then(setData).catch(() => setData({ tracks: [], albums: [], artists: [] })); }, [favs]);
  const empty = !data || (data.tracks.length === 0 && data.albums.length === 0 && data.artists.length === 0);
  return (
    <div>
      <h1 style={{ display: "flex", alignItems: "center", gap: 8 }}><Icon name="heart" size={22} filled /> {t("Favoris")}</h1>
      {!data ? <div className="empty">{t("Chargement…")}</div> : empty ? (
        <div className="empty">{t("Aucun favori pour l’instant — touchez le cœur sur un titre, un album ou un artiste.")}</div>
      ) : (
        <>
          {data.albums.length > 0 && (
            <>
              <h2>{t("Albums")} ({data.albums.length})</h2>
              <div className="grid">
                {data.albums.map((a) => (
                  <div key={a.id} className="card" onClick={() => openAlbum({ id: a.id, title: a.title, artist: a.artist })}>
                    <div className="cover">{a.cover_track_id ? <img src={api.coverUrl(a.cover_track_id)} alt="" loading="lazy" /> : <Icon name="note" size={44} />}
                      <PinButton on onToggle={() => onTogglePin("album", a.id)} />
                      <PinButton on={pins.albums.includes(a.id)} onToggle={() => onTogglePin("album", a.id)} />
                      <FavHeart on onToggle={() => onToggleFav("album", a.id)} />
                    </div>
                    <div className="t">{a.title}</div>
                    <div className="s">{a.artist}{a.year ? ` · ${a.year}` : ""} · {cl(a.track_count, "titre", "titres")}</div>
                  </div>
                ))}
              </div>
            </>
          )}
          {data.artists.length > 0 && (
            <>
              <h2>{t("Artistes")} ({data.artists.length})</h2>
              <table className="tracks"><tbody>
                {data.artists.map((a) => (
                  <tr key={a.id} className="link" onClick={() => openArtist({ id: a.id, name: a.name })}>
                    <td>{a.name}</td>
                    <td className="meta"><FavHeart on onToggle={() => onToggleFav("artist", a.id)} /> {cl(a.album_count, "album", "albums")}</td>
                  </tr>
                ))}
              </tbody></table>
            </>
          )}
          {data.tracks.length > 0 && (
            <>
              <h2>{t("Titres")} ({data.tracks.length})</h2>
              <TrackTable tracks={data.tracks} favs={favs} onToggleFav={onToggleFav} playTrack={playTrack} playAll={playAll} />
            </>
          )}
        </>
      )}
    </div>
  );
}

function fmtFormat(f: NonNullable<Track["format"]>): string {
  if (f.kind === "dsd") {
    const mult = f.sample_rate && f.sample_rate % 44100 === 0 ? f.sample_rate / 44100 : 0;
    return `DSD${64 * mult} · ${f.channels ?? 2} ch`;
  }
  const sr = f.sample_rate ? `${(f.sample_rate / 1000).toFixed(1)}k` : "";
  const bits = f.bits ? `${f.bits}b` : "";
  const ch = f.channels ? `${f.channels}ch` : "";
  return [f.kind, sr, bits, ch].filter(Boolean).join(" · ");
}

function TrackTable({ tracks, current, playTrack, playAll, albumCover, favs, onToggleFav, loading }: { tracks: Track[]; current?: string | null; playTrack: (t: Track) => void; playAll: (t: Track[]) => void; albumCover?: string | null; favs?: Favorites; onToggleFav?: (kind: FavKind, id: string) => void; loading?: boolean }) {
  if (loading) return <div className="empty">{t("Chargement…")}</div>;
  if (tracks.length === 0) return <div className="empty">Aucun titre.</div>;
  return (
    <table className="tracks">
      <thead><tr><th>#</th><th>Titre</th><th>Artiste</th><th>Album</th><th>Format</th><th>Durée</th><th></th></tr></thead>
      <tbody>
        {tracks.map((tr, i) => (
          <tr key={tr.id} className={current === tr.id ? "playing" : ""} onDoubleClick={() => playTrack(tr)} onClick={() => playAll(tracks.slice(i))}>
            <td className="num">{albumCover && i === 0 ? <img src={albumCover} alt="" style={{ width: 28, height: 28, borderRadius: 4, verticalAlign: "middle" }} /> : i + 1}</td>
            <td>{tr.title}</td><td>{tr.artist}</td><td>{tr.album}</td>
            <td><span className="badge">{tr.format ? fmtFormat(tr.format) : ""}</span></td>
            <td className="num-time">{tr.duration_secs != null ? fmtTime(tr.duration_secs) : "—"}</td>
            <td className="num">
              {tr.artist && (
                <button className="icon-btn" title={t("Lancer une radio basée sur {artist}", { artist: tr.artist })}
                  onClick={(e) => { e.stopPropagation(); window.dispatchEvent(new CustomEvent("auris:artist-radio", { detail: tr.artist })); }}>
                  <Icon name="radio" size={16} />
                </button>
              )}
              {favs && onToggleFav && <FavHeart on={favs.tracks.includes(tr.id)} onToggle={() => onToggleFav("track", tr.id)} />}
            </td>
          </tr>
        ))}
      </tbody>
    </table>
  );
}

// ---------- Parcourir ----------

type MenuState = { x: number; y: number; entry: BrowseEntry } | null;

/// Pochette d'un dossier (Parcourir): image du dossier (cover.jpg...) ou
/// pochette embarquee du premier fichier audio; repli sur l'icone si 404.
function CoverImg({ uri, icon, size = 44 }: { uri: string; icon: string; size?: number }) {
  const [err, setErr] = useState(false);
  if (err) return <Icon name={icon} size={size} />;
  return <img src={`/api/browse/cover?uri=${encodeURIComponent(uri)}`} alt="" loading="lazy" onError={() => setErr(true)} />;
}

/// Un segment de chemin de parcours : on navigue par URI (les object_id UPnP
/// contiennent des '/', on ne peut pas reconstruire l'URI depuis les noms).
type BrowseSeg = { name: string; uri: string };

function Browse({ sources, src, upnpDevs, playUri, act, say }: { sources: Source[]; src: string | null; upnpDevs: UpnpDevice[]; playUri: (u: string) => void; act: (fn: () => Promise<unknown>, ok?: string) => Promise<void>; say: (m: string) => void }) {
  const [path, setPath] = useState<BrowseSeg[]>([]);
  const [entries, setEntries] = useState<BrowseEntry[]>([]);
  const [loading, setLoading] = useState(false);
  const [grid, setGrid] = useState(true);
  const [menu, setMenu] = useState<MenuState>(null);
  const [menuSub, setMenuSub] = useState<null | "playlists">(null);
  const [playlists, setPlaylists] = useState<Playlist[]>([]);
  const [copyTarget, setCopyTarget] = useState<BrowseEntry | null>(null);
  const id = src ?? sources[0]?.id ?? null;
  // Racine « upnp://<location>/0 » (serveur découvert, #11) ou source configurée.
  const isUpnp = !!id && id.startsWith("upnp://");
  const rootUri = id ? (isUpnp ? id : `src://${id}/`) : null;
  const curUri = path.length > 0 ? path[path.length - 1].uri : rootUri ?? "";

  useEffect(() => { setPath([]); }, [id]);
  useEffect(() => {
    if (!rootUri) return;
    setLoading(true);
    api.browse(curUri).then(setEntries).catch((e) => say(e instanceof Error ? e.message : String(e))).finally(() => setLoading(false));
    setMenu(null);
    setMenuSub(null);
  }, [id, curUri, say]);

  if (!id || !rootUri) return <div className="empty">Aucune source configurée — ajoutez-en une dans « Sources &amp; réglages ».</div>;
  const source = sources.find((s) => s.id === id);
  const upnpDev = upnpDevs.find((d) => id.startsWith(`upnp://${d.location}`));
  const rootName = source?.name ?? upnpDev?.friendly_name ?? t("UPnP/DLNA");

  // Un clic simple ouvre un dossier ou lance un fichier audio (plus de double clic).
  const openEntry = (e: BrowseEntry) => {
    if (e.is_dir) setPath([...path, { name: e.name, uri: e.uri }]);
    else if (e.is_audio) playUri(e.uri);
  };

  const openMenu = (e: React.MouseEvent, entry: BrowseEntry) => {
    e.preventDefault();
    e.stopPropagation();
    setMenu({ x: e.clientX, y: e.clientY, entry });
    setMenuSub(null);
  };

  const closeMenu = () => { setMenu(null); setMenuSub(null); };

  // « Ajouter à une playlist » : sous-menu listant les playlists existantes.
  const openPlaylistsSub = async (entry: BrowseEntry) => {
    setMenuSub("playlists");
    try { setPlaylists(await api.playlists()); }
    catch (e) { say(e instanceof Error ? e.message : String(e)); }
    void entry;
  };

  const addEntryToPlaylist = async (pid: string, pname: string, entry: BrowseEntry) => {
    try {
      const r = await api.playlistAdd(pid, entry.uri);
      say(`Ajouté à « ${pname} » (${r.added} piste${r.added > 1 ? "s" : ""})`);
    } catch (e) { say(e instanceof Error ? e.message : String(e)); }
    closeMenu();
  };

  const createPlaylistFromEntry = async (entry: BrowseEntry) => {
    const name = window.prompt("Nom de la nouvelle liste de lecture :");
    if (!name || !name.trim()) return;
    try {
      // créé la playlist à partir de l'uri, puis on recharge la liste
      await api.addPlaylist(name.trim(), undefined, entry.uri);
      say(`Liste « ${name.trim()} » créée`);
      setPlaylists(await api.playlists());
    } catch (e) { say(e instanceof Error ? e.message : String(e)); }
  };

  return (
    <div>
      <div className="toolbar">
        <div className="crumb">
          <button className="crumb-btn" onClick={() => setPath(path.slice(0, -1))} disabled={path.length === 0} title="Dossier précédent"><Icon name="back" size={16} /></button>
          <button className="crumb-btn" onClick={() => setPath([])} title="Racine de la source"><Icon name="home" size={16} /></button>
          {path.map((p, i) => (
            <span key={i}>
              <span className="crumb-sep">/</span>
              <button className="crumb-btn" onClick={() => setPath(path.slice(0, i + 1))}>{p.name}</button>
            </span>
          ))}
        </div>
        <div className="spacer" />
        <button className="btn ghost sm" onClick={() => act(() => api.play(curUri), "Lecture du dossier…")} title="Lire tout le dossier"><Icon name="play" size={14} filled /> Tout lire</button>
        <button className="btn ghost sm" onClick={() => act(() => api.queueAdd(curUri), "Ajouté à la file d'attente")} title="Ajouter tout le dossier à la file"><Icon name="plus" size={14} /> File</button>
        <div className="seg">
          <button className={`seg-btn icon ${grid ? "active" : ""}`} onClick={() => setGrid(true)} title="Grille"><Icon name="grid" size={16} /></button>
          <button className={`seg-btn icon ${!grid ? "active" : ""}`} onClick={() => setGrid(false)} title="Liste"><Icon name="list" size={16} /></button>
        </div>
      </div>
      <h1><Icon name={source ? kindIconName(source.kind) : "server"} size={28} /> {rootName}{path.length > 0 && <span className="crumb-sep"> — {path.map((p) => p.name).join(" / ")}</span>}</h1>
      {loading && <div className="empty">Chargement…</div>}
      {!loading && grid && (
        <div className="grid">
          {entries.map((e) => (
            <div key={e.uri} className="card" onClick={() => openEntry(e)}
              onContextMenu={(ev) => openMenu(ev, e)} title={e.name}>
              <div className="cover">{e.is_dir ? <CoverImg uri={e.uri} icon="folder" /> : <Icon name="note" size={44} />}</div>
              <div className="t">{e.name}</div>
              <div className="s">{e.is_dir ? "dossier" : `audio${e.size ? ` · ${fmtBytes(e.size)}` : ""}`}</div>
              <button className="icon-btn card-menu" title="Options" onClick={(ev) => openMenu(ev, e)}><Icon name="dots" size={18} /></button>
            </div>
          ))}
        </div>
      )}
      {!loading && !grid && entries.map((e) => (
        <div key={e.uri} className="browse-row" onClick={() => openEntry(e)} onContextMenu={(ev) => openMenu(ev, e)}>
          <span className="row-ico">{e.is_dir ? <CoverImg uri={e.uri} icon="folder" size={20} /> : <Icon name="note" size={20} />}</span>
          <span className="name">{e.name}</span>
          <span className="meta">{e.is_audio && e.format ? fmtFormat(e.format) : ""}{e.size ? ` · ${fmtBytes(e.size)}` : ""}</span>
          <button className="icon-btn" onClick={(ev) => openMenu(ev, e)}><Icon name="dots" size={18} /></button>
        </div>
      ))}
      {!loading && entries.length === 0 && <div className="empty">Dossier vide.</div>}

      {menu && (
        <>
          <div className="menu-overlay" onClick={closeMenu} onContextMenu={(e) => { e.preventDefault(); closeMenu(); }} />
          <div className="ctx-menu" style={{ left: menu.x, top: menu.y }}>
            <div className="ctx-title">{menu.entry.name}</div>
            {menuSub === "playlists" ? (
              <>
                <div className="ctx-item" onClick={() => setMenuSub(null)}><Icon name="back" size={16} /> Retour</div>
                {playlists.length === 0 && <div className="ctx-item" style={{ color: "var(--text-dim)" }}>Aucune liste de lecture</div>}
                {playlists.map((p) => (
                  <div key={p.id} className="ctx-item" onClick={() => addEntryToPlaylist(p.id, p.name, menu.entry)}>
                    <Icon name="playlist" size={16} /> {p.name}
                  </div>
                ))}
                <div className="ctx-item" onClick={() => createPlaylistFromEntry(menu.entry)}><Icon name="plus" size={16} /> Nouvelle liste…</div>
              </>
            ) : (
              <>
                <div className="ctx-item" onClick={() => { playUri(menu.entry.uri); closeMenu(); }}><Icon name="play" size={16} filled /> Lire maintenant</div>
                <div className="ctx-item" onClick={() => { act(() => api.queueAddNext(menu.entry.uri), " lira juste après la piste en cours"); closeMenu(); }}><Icon name="next" size={16} /> Lire ensuite</div>
                <div className="ctx-item" onClick={() => { act(() => api.queueAdd(menu.entry.uri), "Ajouté à la file d'attente"); closeMenu(); }}><Icon name="plus" size={16} /> Ajouter à la file d'attente</div>
                <div className="ctx-item" onClick={() => { act(async () => { await api.clearQueue(); await api.play(menu.entry.uri); }, "File vidée, lecture lancée"); closeMenu(); }}><Icon name="queue" size={16} /> Vider la file et lire</div>
                <div className="ctx-item" onClick={() => openPlaylistsSub(menu.entry)}><Icon name="playlist" size={16} /> Ajouter à une playlist…</div>
                {!isUpnp && (
                  <>
                    <div className="ctx-item" onClick={() => { act(async () => { await api.scan(id); }, "Mise à jour du dossier lancée"); closeMenu(); }}><Icon name="refresh" size={16} /> Mettre à jour le dossier</div>
                    <div className="ctx-item" onClick={() => { window.location.href = api.exportUrl(menu.entry.uri, menu.entry.name.replace(/\.[^.]+$/, "")); closeMenu(); }}><Icon name="download" size={16} /> Télécharger (ZIP)</div>
                    <div className="ctx-item" onClick={() => { setCopyTarget(menu.entry); closeMenu(); }}><Icon name="copy" size={16} /> Copier vers une autre source…</div>
                  </>
                )}
              </>
            )}
          </div>
        </>
      )}

      {copyTarget && (
        <CopyModal entry={copyTarget} sources={sources} currentSrcId={id} onClose={() => setCopyTarget(null)} say={say} />
      )}
    </div>
  );
}

function CopyModal({ entry, sources, currentSrcId, onClose, say }: { entry: BrowseEntry; sources: Source[]; currentSrcId: string; onClose: () => void; say: (m: string) => void }) {
  const writable = sources.filter((s) => s.enabled && ["local", "usb", "smb"].includes(s.kind) && s.id !== currentSrcId);
  const [dest, setDest] = useState<string>(writable[0]?.id ?? "");
  const [destPath, setDestPath] = useState("");
  const [busy, setBusy] = useState(false);
  const submit = async () => {
    if (!dest) return;
    setBusy(true);
    try {
      const to = `src://${dest}/${destPath.trim().replace(/^\/+|\/+$/g, "")}`;
      const r: CopyReport = await api.copy(entry.uri, to);
      if (r.errors.length > 0) say(`Copie terminée avec ${r.errors.length} erreur(s): ${r.errors[0]}`);
      else say(`Copié : ${r.files} fichier(s), ${fmtBytes(r.bytes)}`);
      onClose();
    } catch (e) { say(e instanceof Error ? e.message : String(e)); }
    finally { setBusy(false); }
  };
  return (
    <div className="modal-bg" onClick={onClose}>
      <div className="modal wide" onClick={(e) => e.stopPropagation()}>
        <div className="modal-head">
          <h2>Copier « {entry.name} » vers…</h2>
          <button className="icon-btn" onClick={onClose}><Icon name="close" size={20} /></button>
        </div>
        <label>Source destination
          <select value={dest} onChange={(e) => setDest(e.target.value)}>
            {writable.length === 0 && <option value="">aucune source inscriptible</option>}
            {writable.map((s) => <option key={s.id} value={s.id}>{s.name} ({kindLabel(s.kind)})</option>)}
          </select>
        </label>
        <label>Dossier destination (relatif, vide = racine)
          <input type="text" value={destPath} onChange={(e) => setDestPath(e.target.value)} placeholder="ex: import/2026" />
        </label>
        <div className="modal-foot">
          <button className="btn ghost" onClick={onClose}>Annuler</button>
          <button className="btn" disabled={busy || !dest} onClick={submit}>{busy ? "Copie…" : "Copier"}</button>
        </div>
      </div>
    </div>
  );
}

// ---------- Radios web ----------

type MenuTarget =
  | { kind: "entry"; entry: RadioEntry }
  | { kind: "station"; station: DirStation };
type RadioMenuState = { x: number; y: number; target: MenuTarget } | null;

type DirData = DirTag[] | DirCountry[] | DirStation[] | DirSection[];

// ---------- Plugins communautaires (#8) ----------

function PluginsView({ act, say }: { act: (fn: () => Promise<unknown>, ok?: string) => Promise<void>; say: (m: string) => void }) {
  const [plugins, setPlugins] = useState<PluginInfo[] | null>(null);
  const [path, setPath] = useState("");
  const [testResult, setTestResult] = useState<Record<string, string>>({});

  const reload = useCallback(async () => {
    try { setPlugins(await api.plugins()); }
    catch (e) { say(e instanceof Error ? e.message : String(e)); }
  }, [say]);

  useEffect(() => { reload(); }, [reload]);

  const catLabel = (c: string) =>
    c === "source" ? t("Source") : c === "output" ? t("Sortie audio") : c === "metadata" ? t("Métadonnées") : c === "ui" ? t("Interface") : c;

  return (
    <div className="plugins-view">
      <div className="section-head">
        <h1><Icon name="puzzle" size={28} /> {t("Plugins")}</h1>
        <button className="btn ghost sm" onClick={reload}><Icon name="refresh" size={16} /> {t("Actualiser")}</button>
      </div>

      <p className="empty" style={{ marginTop: 0 }}>
        {t("Plugins communautaires Auris (manifeste auris-plugin.toml + binaire, JSON-RPC sur stdio). Installez depuis un répertoire local contenant le manifeste.")}
      </p>

      <div className="section-head">
        <h2>{t("Installer depuis un dossier local")}</h2>
        <div className="mr-newgroup">
          <input type="text" value={path} placeholder={t("Chemin du dossier du plugin (ex: C:\\plugins\\mon-plugin)")} style={{ width: 320 }}
            onChange={(e) => setPath(e.target.value)}
            onKeyDown={(e) => { if (e.key === "Enter" && path.trim()) { act(async () => { await api.pluginInstall(path.trim()); setPath(""); await reload(); }, t("Plugin installé")); } }} />
          <button className="btn sm" disabled={!path.trim()}
            onClick={() => act(async () => { await api.pluginInstall(path.trim()); setPath(""); await reload(); }, t("Plugin installé"))}>
            <Icon name="plus" size={16} /> {t("Installer")}
          </button>
        </div>
      </div>

      {plugins === null && <div className="empty">{t("Chargement…")}</div>}
      {plugins?.length === 0 && <div className="empty">{t("Aucun plugin installé.")}</div>}

      <div className="mr-peers" style={{ flexDirection: "column", alignItems: "stretch" }}>
        {plugins?.map((p) => (
          <div className="mr-peer" key={p.id} style={{ justifyContent: "space-between" }}>
            <span className="row-ico"><Icon name="puzzle" size={20} /></span>
            <div style={{ flex: 1 }}>
              <div className="mr-name">
                {p.name} <span className="badge">{catLabel(p.category)}</span>{" "}
                <span className="badge">{p.version}</span>{" "}
                {p.running && <span className="badge live">{t("en cours")}</span>}
                {!p.enabled && <span className="badge">{t("désactivé")}</span>}
              </div>
              <div className="mr-base">
                {p.id} · {p.license}{p.authors.length ? ` · ${p.authors.join(", ")}` : ""}
                {p.description ? ` — ${p.description}` : ""}
              </div>
              <div className="mr-base" style={{ opacity: 0.7 }}>{p.dir}</div>
              {p.last_error && <div className="mr-base" style={{ color: "#ff6b6b" }}>{p.last_error}</div>}
              {testResult[p.id] && (
                <pre style={{ margin: "6px 0 0", fontSize: 11, maxHeight: 120, overflow: "auto", background: "var(--bg)", padding: 8, borderRadius: 8 }}>
                  {testResult[p.id]}
                </pre>
              )}
            </div>
            <div style={{ display: "flex", gap: 8, alignItems: "center" }}>
              <Switch checked={p.enabled} label={t("Activer / désactiver")}
                onChange={(v) => act(async () => { v ? await api.pluginEnable(p.id) : await api.pluginDisable(p.id); await reload(); }, v ? t("Plugin activé") : t("Plugin désactivé"))} />
              <button className="btn ghost sm" title={t("Tester le plugin (manifest.get + capabilities.list)")}
                onClick={() => act(async () => {
                  const r = await api.pluginTest(p.id) as Record<string, unknown>;
                  setTestResult((tr) => ({ ...tr, [p.id]: JSON.stringify(r, null, 2).slice(0, 2000) }));
                })}>
                <Icon name="flame" size={15} /> {t("Tester")}
              </button>
              <button className="btn ghost sm" title={t("Désinstaller le plugin")}
                onClick={() => act(async () => { await api.pluginUninstall(p.id); await reload(); }, t("Plugin désinstallé"))}>
                <Icon name="trash" size={15} />
              </button>
            </div>
          </div>
        ))}
      </div>
    </div>
  );
}

function MultiroomView({ state, act, say }: { state: PlayerState | null; act: (fn: () => Promise<unknown>, ok?: string) => Promise<void>; say: (m: string) => void }) {
  const [mr, setMr] = useState<MultiroomState | null>(null);
  const [newGroup, setNewGroup] = useState("");

  const reload = useCallback(async () => {
    try { setMr(await api.multiroomState()); }
    catch (e) { say(e instanceof Error ? e.message : String(e)); }
  }, [say]);

  useEffect(() => {
    reload();
    const iv = setInterval(reload, 10000);
    return () => clearInterval(iv);
  }, [reload]);

  if (!mr) return <div className="empty">{t("Chargement…")}</div>;

  const memberName = (id: string) =>
    id === mr.self_id ? `${mr.self_name} (${t("cet appareil")})` : (mr.peers.find((p) => p.id === id)?.name ?? id.slice(0, 8));

  const candidates = [mr.self_id, ...mr.peers.map((p) => p.id)];

  const report = (r: { ok: string[]; failed: string[] }, label: string) =>
    say(`${label} — OK: ${r.ok.join(", ") || "—"}${r.failed.length ? ` | échec: ${r.failed.join(", ")}` : ""}`);

  return (
    <div className="multiroom">
      <div className="section-head">
        <h1><Icon name="group" size={28} /> {t("Multiroom")}</h1>
        <button className="btn ghost sm" onClick={reload}><Icon name="refresh" size={16} /> {t("Actualiser")}</button>
      </div>

      <div className="mr-peers">
        <div className="mr-peer self">
          <span className="row-ico"><Icon name="speaker" size={20} /></span>
          <div><div className="mr-name">{mr.self_name}</div><div className="mr-base">{t("cet appareil")}</div></div>
        </div>
        {mr.peers.map((p) => (
          <div className="mr-peer" key={p.id}>
            <span className="row-ico"><Icon name="server" size={20} /></span>
            <div><div className="mr-name">{p.name}</div><div className="mr-base">{p.base}</div></div>
          </div>
        ))}
        {mr.peers.length === 0 && (
          <div className="empty">{t("Aucun autre Auris détecté sur le réseau (balise multicast 239.255.255.251:3011).")}</div>
        )}
      </div>

      <div className="section-head">
        <h2>{t("Groupes")}</h2>
        <div className="mr-newgroup">
          <input type="text" value={newGroup} placeholder={t("Nom du groupe")} style={{ width: 200 }}
            onChange={(e) => setNewGroup(e.target.value)}
            onKeyDown={(e) => { if (e.key === "Enter" && newGroup.trim()) { act(async () => { await api.mrCreateGroup(newGroup.trim()); setNewGroup(""); await reload(); }, t("Groupe créé")); } }} />
          <button className="btn sm" disabled={!newGroup.trim()}
            onClick={() => act(async () => { await api.mrCreateGroup(newGroup.trim()); setNewGroup(""); await reload(); }, t("Groupe créé"))}>
            <Icon name="plus" size={16} /> {t("Créer")}
          </button>
        </div>
      </div>

      {mr.groups.length === 0 && <div className="empty">{t("Aucun groupe — créez-en un pour synchroniser plusieurs Auris.")}</div>}
      {mr.groups.map((g: MrGroup) => (
        <div className="mr-group" key={g.id}>
          <div className="mr-group-head">
            <strong>{g.name}</strong>
            <span className="mr-group-actions">
              <button className="btn ghost sm" title={t("Lire la piste en cours sur tout le groupe")}
                disabled={!state?.current}
                onClick={() => act(async () => report(await api.mrPlay(g.id, state?.current?.uri, state?.position_secs), t("Lecture groupe")), t("Lecture envoyée"))}>
                <Icon name="play" size={15} /> {t("Jouer")}
              </button>
              <button className="btn ghost sm" onClick={() => act(async () => report(await api.mrPause(g.id), t("Pause groupe")), t("Pause envoyée"))}>
                <Icon name="pause" size={15} />
              </button>
              <button className="btn ghost sm" onClick={() => act(async () => report(await api.mrStop(g.id), t("Stop groupe")), t("Stop envoyé"))}>
                <Icon name="stop" size={15} />
              </button>
              <button className="btn ghost sm" title={t("Supprimer le groupe")}
                onClick={() => act(async () => { await api.mrDeleteGroup(g.id); await reload(); }, t("Groupe supprimé"))}>
                <Icon name="trash" size={15} />
              </button>
            </span>
          </div>
          <div className="mr-members">
            {g.members.map((m) => (
              <div className="mr-member" key={m}>
                <span className="row-ico"><Icon name="speaker" size={17} /></span>
                <span className="mr-name">{memberName(m)}</span>
                <input type="range" min={0} max={100} defaultValue={state?.volume ?? 80} style={{ width: 90 }}
                  title={t("Volume de cette zone")}
                  onChange={(e) => api.mrVolume(g.id, Number(e.target.value), m).then((r) => report(r, t("Volume zone"))).catch((e2) => say(String(e2)))} />
                <button className="icon-btn" title={t("Retirer du groupe")}
                  onClick={() => act(async () => { await api.mrSetMember(g.id, m, false); await reload(); }, t("Membre retiré"))}>
                  <Icon name="close" size={14} />
                </button>
              </div>
            ))}
            <select className="mr-add" defaultValue="" onChange={(e) => { const v = e.target.value; if (v) act(async () => { await api.mrSetMember(g.id, v, true); await reload(); }, t("Membre ajouté")); e.target.value = ""; }}>
              <option value="">{t("+ ajouter une zone…")}</option>
              {candidates.filter((c) => !g.members.includes(c)).map((c) => <option key={c} value={c}>{memberName(c)}</option>)}
            </select>
          </div>
          <div className="mr-groupvol">
            <span>{t("Volume du groupe")}</span>
            <input type="range" min={0} max={100} defaultValue={state?.volume ?? 80} style={{ width: 160 }}
              onChange={(e) => api.mrVolume(g.id, Number(e.target.value)).then((r) => report(r, t("Volume groupe"))).catch((e2) => say(String(e2)))} />
          </div>
        </div>
      ))}
    </div>
  );
}

function Radios({ state, act, say, pins, onTogglePin }: { state: PlayerState | null; act: (fn: () => Promise<unknown>, ok?: string) => Promise<void>; say: (m: string) => void; pins: Pins; onTogglePin: (kind: PinKind, id: string) => void }) {
  const [list, setList] = useState<RadioList | null>(null);
  const [menu, setMenu] = useState<RadioMenuState>(null);
  const [view, setView] = useState("root");
  const [dir, setDir] = useState<{ view: string; data: DirData } | null>(null);
  const [dirLoading, setDirLoading] = useState(false);
  const [showAdd, setShowAdd] = useState(false);
  const [name, setName] = useState("");
  const [url, setUrl] = useState("");
  const [adding, setAdding] = useState(false);
  const [brokenFav, setBrokenFav] = useState<Set<string>>(new Set());
  // podcasts (#81)
  const [podQuery, setPodQuery] = useState("");
  const [podSearch, setPodSearch] = useState("");
  const [podResults, setPodResults] = useState<PodcastShow[] | null>(null);
  const [podSubs, setPodSubs] = useState<PodcastShow[]>([]);
  const [podFeed, setPodFeed] = useState<{ url: string; data: PodcastFeed } | null>(null);
  const [podLoading, setPodLoading] = useState(false);

  const reloadSubs = useCallback(async () => {
    try { setPodSubs(await api.podcastSubscriptions()); }
    catch (e) { say(e instanceof Error ? e.message : String(e)); }
  }, [say]);

  // recherche de podcasts (#81) — lancée quand on ouvre la vue ou on soumet
  useEffect(() => {
    if (view !== "podcasts") return;
    reloadSubs();
    let cancelled = false;
    setPodLoading(true);
    const run = async () => {
      try {
        const res = await api.podcasts(podSearch, 30);
        if (!cancelled) setPodResults(res);
      } catch (e) {
        if (!cancelled) say(e instanceof Error ? e.message : String(e));
      } finally {
        if (!cancelled) setPodLoading(false);
      }
    };
    run();
    return () => { cancelled = true; };
  }, [view, podSearch, say]);

  // chargement du flux RSS d'une émission (#81)
  useEffect(() => {
    if (!view.startsWith("podshow:")) return;
    const feedUrl = decodeURIComponent(view.slice(8));
    let cancelled = false;
    setPodLoading(true);
    api.podcastFeed(feedUrl)
      .then((data) => { if (!cancelled) setPodFeed({ url: feedUrl, data }); })
      .catch((e) => { if (!cancelled) say(e instanceof Error ? e.message : String(e)); })
      .finally(() => { if (!cancelled) setPodLoading(false); });
    return () => { cancelled = true; };
  }, [view, say]);

  const reload = useCallback(async () => {
    try { setList(await api.radios()); }
    catch (e) { say(e instanceof Error ? e.message : String(e)); }
  }, [say]);

  useEffect(() => { reload(); }, [reload]);

  // chargement de la sous-vue annuaire (top/popular/genres/pays/recherche…)
  useEffect(() => {
    if (view === "root" || view === "pinned" || view === "fav" || view === "builtin" || view === "sources" || view === "podcasts" || view.startsWith("podshow:")) return;
    let cancelled = false;
    setDirLoading(true);
    const load = async () => {
      try {
        let data: DirData;
        if (view === "selection") data = await api.radiosSelection();
        else if (view === "genres") data = await api.radiosDirGenres();
        else if (view === "countries") data = await api.radiosDirCountries();
        else if (view === "top") data = await api.radiosDirStations("top");
        else if (view === "top500") data = await api.radiosDirStations("top", "", 500);
        else if (view === "popular") data = await api.radiosDirStations("popular");
        else if (view.startsWith("local:")) data = await api.radiosDirStations("country", view.slice(6));
        else if (view.startsWith("genre:")) data = await api.radiosDirStations("genre", view.slice(6));
        else if (view.startsWith("country:")) data = await api.radiosDirStations("country", view.slice(8));
        else if (view.startsWith("search:")) data = await api.radiosDirStations("search", view.slice(7));
        else return;
        if (!cancelled) setDir({ view, data });
      } catch (e) {
        if (!cancelled) say(e instanceof Error ? e.message : String(e));
      } finally {
        if (!cancelled) setDirLoading(false);
      }
    };
    load();
    return () => { cancelled = true; };
  }, [view, say]);

  const submit = async () => {
    if (!name.trim() || !url.trim() || adding) return;
    setAdding(true);
    try {
      setList(await api.addRadio(name.trim(), url.trim()));
      say(`Radio « ${name.trim()} » ajoutée aux favoris`);
      setName(""); setUrl(""); setShowAdd(false);
    } catch (e) { say(e instanceof Error ? e.message : String(e)); }
    finally { setAdding(false); }
  };

  const play = (r: RadioEntry) => act(() => api.playRadio(r.id), `Lecture de « ${r.name} »`);
  const playStation = (st: DirStation) => act(() => api.playRadioUrl(st.name, st.url), `Lecture de « ${st.name} »`);
  const targetUrl = (t: MenuTarget) => (t.kind === "entry" ? t.entry.url : t.station.url);
  const targetName = (t: MenuTarget) => (t.kind === "entry" ? t.entry.name : t.station.name);
  /// ID d'épingle d'une radio (#70) : id d'entrée, ou URL pour une station de l'annuaire.
  const targetPinId = (t: MenuTarget) => (t.kind === "entry" ? t.entry.id : t.station.url);
  const isPinned = (t: MenuTarget) => pins.radios.includes(targetPinId(t));
  const playTarget = (t: MenuTarget) => (t.kind === "entry" ? play(t.entry) : playStation(t.station));
  const isFavUrl = (u: string) => list?.favorites.some((f) => f.url === u) ?? false;
  const addFav = async (t: MenuTarget) => {
    try { setList(await api.addRadio(targetName(t), targetUrl(t))); say(`« ${targetName(t)} » ajouté aux favoris`); }
    catch (e) { say(e instanceof Error ? e.message : String(e)); }
  };
  const delFav = (r: RadioEntry) => act(async () => { setList(await api.deleteRadio(r.id)); }, `« ${r.name} » retirée des favoris`);

  const openMenu = (e: React.MouseEvent, target: MenuTarget) => {
    e.preventDefault(); e.stopPropagation();
    setMenu({ x: e.clientX, y: e.clientY, target });
  };
  const closeMenu = () => setMenu(null);

  const row = (r: RadioEntry) => {
    const current = state?.current?.uri === r.url;
    const liveTitle = current && state?.is_live ? state.stream_title : null;
    return (
      <div key={r.id} className={`browse-row ${current ? "playing" : ""}`} onClick={() => play(r)}
        onContextMenu={(ev) => openMenu(ev, { kind: "entry", entry: r })} title={r.url}>
        <span className="row-ico"><Icon name="radio" size={20} /></span>
        <span className="name">{r.name}</span>
        <span className="meta">{liveTitle ?? (current ? t("en direct") : r.url)}</span>
        <button className="icon-btn" title={t("Options")} onClick={(ev) => openMenu(ev, { kind: "entry", entry: r })}><Icon name="dots" size={18} /></button>
      </div>
    );
  };

  const stationRow = (st: DirStation) => {
    const current = state?.current?.uri === st.url;
    const liveTitle = current && state?.is_live ? state.stream_title : null;
    const parts = [
      st.codec ? `${st.codec.toUpperCase()}${st.bitrate > 0 ? ` ${st.bitrate}k` : ""}` : "",
      st.country,
      st.tags.split(",").map((t) => t.trim()).filter(Boolean).slice(0, 3).join(", "),
    ].filter(Boolean);
    const favBroken = st.favicon !== "" && brokenFav.has(st.favicon);
    return (
      <div key={st.id || st.url} className={`browse-row ${current ? "playing" : ""}`} onClick={() => playStation(st)}
        onContextMenu={(ev) => openMenu(ev, { kind: "station", station: st })} title={st.url}>
        <span className="row-ico">
          {st.favicon && !favBroken
            ? <img src={st.favicon} alt="" style={{ width: 20, height: 20, borderRadius: 4, objectFit: "cover" }}
                onError={() => setBrokenFav((s) => new Set(s).add(st.favicon))} />
            : <Icon name="radio" size={20} />}
        </span>
        <span className="name">{st.name}</span>
        <span className="meta">{liveTitle ?? (current ? t("en direct") : parts.join(" · "))}</span>
        <button className="icon-btn" title={t("Options")} onClick={(ev) => openMenu(ev, { kind: "station", station: st })}><Icon name="dots" size={18} /></button>
      </div>
    );
  };

  const tagRow = (tg: DirTag) => (
    <div key={tg.name} className="browse-row" onClick={() => setView(`genre:${tg.name}`)} title={tg.name}>
      <span className="row-ico"><Icon name="note" size={20} /></span>
      <span className="name">{tg.name}</span>
      <span className="meta">{t("{count} radio(s)", { count: tg.count })}</span>
    </div>
  );

  const countryRow = (c: DirCountry) => (
    <div key={c.code} className="browse-row" onClick={() => setView(`country:${c.code}`)} title={c.name}>
      <span className="row-ico"><Icon name="globe" size={20} /></span>
      <span className="name">{c.name}</span>
      <span className="meta">{c.code} · {t("{count} radio(s)", { count: c.count })}</span>
    </div>
  );

  // ---------- Podcasts (#81) ----------

  const subscribe = async (s: PodcastShow) => {
    try {
      setPodSubs(await api.podcastSubscribe({ feed_url: s.feed_url, title: s.title, artwork: s.artwork, author: s.author }));
      setPodResults((rs) => rs?.map((x) => (x.feed_url === s.feed_url ? { ...x, subscribed: true } : x)) ?? rs);
      say(t("« {title} » ajouté aux abonnements", { title: s.title }));
    } catch (e) { say(e instanceof Error ? e.message : String(e)); }
  };
  const unsubscribe = async (s: PodcastShow) => {
    try {
      setPodSubs(await api.podcastUnsubscribe(s.id));
      setPodResults((rs) => rs?.map((x) => (x.feed_url === s.feed_url ? { ...x, subscribed: false } : x)) ?? rs);
      say(t("« {title} » retiré des abonnements", { title: s.title }));
    } catch (e) { say(e instanceof Error ? e.message : String(e)); }
  };
  const playEpisode = (ep: PodcastEpisode, show: string) =>
    act(() => api.playPodcastEpisode({ url: ep.url, title: ep.title, show, duration_secs: ep.duration_secs }), `Lecture de « ${ep.title} »`);

  const showRow = (s: PodcastShow) => (
    <div key={s.id} className="browse-row" onClick={() => setView(`podshow:${encodeURIComponent(s.feed_url)}`)} title={s.feed_url}>
      <span className="row-ico">
        {s.artwork ? <img src={s.artwork} alt="" style={{ width: 20, height: 20, borderRadius: 4, objectFit: "cover" }} /> : <Icon name="podcast" size={20} />}
      </span>
      <span className="name">{s.title}</span>
      <span className="meta">{s.author}</span>
      <button className="icon-btn" title={s.subscribed ? t("Se désabonner") : t("S'abonner")}
        onClick={(ev) => { ev.stopPropagation(); s.subscribed ? unsubscribe(s) : subscribe(s); }}>
        <Icon name="heart" size={18} filled={s.subscribed} />
      </button>
    </div>
  );

  const episodeRow = (ep: PodcastEpisode, show: string) => {
    const current = state?.current?.uri === ep.url;
    const dur = ep.duration_secs ? fmtTime(ep.duration_secs) : "";
    return (
      <div key={ep.id} className={`browse-row ${current ? "playing" : ""}`} onClick={() => playEpisode(ep, show)} title={ep.url}>
        <span className="row-ico">
          {ep.image ? <img src={ep.image} alt="" style={{ width: 20, height: 20, borderRadius: 4, objectFit: "cover" }} /> : <Icon name="play" size={20} />}
        </span>
        <span className="name">{ep.title}</span>
        <span className="meta">{[ep.pub_date, dur].filter(Boolean).join(" · ")}{current ? ` · ${t("en lecture")}` : ""}</span>
        <button className="icon-btn" title={t("Lire")} onClick={(ev) => { ev.stopPropagation(); playEpisode(ep, show); }}><Icon name="play" size={18} /></button>
      </div>
    );
  };

  const podcastsView = () => (
    <>
      <form className="browse-row" style={{ cursor: "default" }} onSubmit={(e) => { e.preventDefault(); setPodSearch(podQuery.trim()); }}>
        <span className="row-ico"><Icon name="search" size={20} /></span>
        <input
          value={podQuery}
          onChange={(e) => setPodQuery(e.target.value)}
          placeholder={t("Rechercher un podcast…")}
          style={{ flex: 1, background: "transparent", border: "none", outline: "none", color: "inherit", font: "inherit" }}
        />
        <button className="icon-btn" type="submit" title={t("Rechercher")}><Icon name="search" size={18} /></button>
      </form>
      {podLoading && <div className="empty">{t("Chargement…")}</div>}
      {!podLoading && podSearch === "" && podSubs.length > 0 && (
        <>
          <h3 style={{ margin: "14px 4px 6px" }}>{t("Mes abonnements")}</h3>
          {podSubs.map(showRow)}
        </>
      )}
      {!podLoading && podSearch !== "" && podResults !== null && (
        podResults.length ? podResults.map(showRow) : <div className="empty">{t("Aucun podcast trouvé.")}</div>
      )}
      {!podLoading && podSearch === "" && podSubs.length === 0 && (
        <div className="empty">{t("Recherchez un podcast ou collez un flux RSS via « S'abonner » sur une émission.")}</div>
      )}
    </>
  );

  const podshowView = () => {
    const feed = podFeed && podFeed.url === decodeURIComponent(view.slice(8)) ? podFeed.data : null;
    if (podLoading || !feed) return <div className="empty">{t("Chargement du flux…")}</div>;
    return (
      <>
        {feed.artwork && <img src={feed.artwork} alt="" style={{ width: 72, height: 72, borderRadius: 8, objectFit: "cover", margin: "4px" }} />}
        {feed.episodes.length === 0
          ? <div className="empty">{t("Aucun épisode dans ce flux.")}</div>
          : feed.episodes.map((ep) => episodeRow(ep, feed.title))}
      </>
    );
  };

  const viewTitle = () => {
    if (view === "selection") return t("Sélection Auris");
    if (view === "pinned") return t("Radios épinglées");
    if (view === "fav") return t("Mes radios");
    if (view === "top") return t("Top radios");
    if (view === "top500") return t("Top 500 radios");
    if (view.startsWith("local:")) return t("Radios locales ({code})", { code: view.slice(6) });
    if (view === "builtin") return t("Radios par défaut");
    if (view === "sources") return t("Sources radio");
    if (view === "popular") return t("Radios populaires");
    if (view === "genres") return t("Par genre");
    if (view === "countries") return t("Par pays");
    if (view.startsWith("genre:")) return t("Genre : {name}", { name: view.slice(6) });
    if (view.startsWith("country:")) return t("Pays : {name}", { name: view.slice(8) });
    if (view.startsWith("search:")) return t("Recherche : {q}", { q: view.slice(7) });
    if (view === "podcasts") return t("Podcasts");
    if (view.startsWith("podshow:")) return podFeed?.url === decodeURIComponent(view.slice(8)) ? podFeed.data.title : t("Podcast");
    return t("Radios");
  };

  /// Code pays du navigateur (fr-FR -> FR) pour la tuile « Radios locales ».
  const localCountryCode = () => {
    try {
      const region = new Intl.Locale(navigator.language || "en").maximize().region;
      return (region || "").toUpperCase();
    } catch {
      return "";
    }
  };

  const tiles = [
    { id: "selection", icon: "sparkles", label: t("Sélection Auris"), sub: t("sélection éditoriale") },
    { id: "pinned", icon: "pin", label: t("Épinglées"), sub: t("{count} radio(s)", { count: pins.radios.length }) },
    { id: "fav", icon: "heart", label: t("Mes radios"), sub: t("{count} radio(s)", { count: list?.favorites.length ?? 0 }) },
    { id: "builtin", icon: "radio", label: t("Radios par défaut"), sub: t("{count} radio(s)", { count: list?.builtin.length ?? 0 }) },
    { id: "sources", icon: "server", label: t("Sources radio"), sub: t("{count} source(s)", { count: list?.sources.length ?? 0 }) },
    { id: "top", icon: "star", label: t("Top radios"), sub: t("les mieux notées") },
    { id: "top500", icon: "trophy", label: t("Top 500 radios"), sub: t("les mieux votées") },
    { id: "local", icon: "marker", label: t("Radios locales"), sub: localCountryCode() ? t("pays : {code}", { code: localCountryCode() }) : t("votre pays") },
    { id: "popular", icon: "flame", label: t("Radios populaires"), sub: t("les plus écoutées") },
    { id: "genres", icon: "note", label: t("Par genre"), sub: t("rock, jazz, classique…") },
    { id: "countries", icon: "globe", label: t("Par pays"), sub: t("du monde entier") },
    { id: "__artist", icon: "user", label: t("Radio d'artiste…"), sub: t("basée sur un artiste") },
    { id: "__search", icon: "search", label: t("Rechercher"), sub: t("par nom de radio") },
    { id: "__add", icon: "plus", label: t("Ajouter une radio"), sub: t("URL de flux personnalisée") },
    { id: "podcasts", icon: "podcast", label: t("Podcasts"), sub: t("recherche & abonnements") },
  ];

  const onTile = (id: string) => {
    if (id === "__artist") {
      const a = window.prompt(t("Radio basée sur quel artiste ?"));
      if (a && a.trim()) {
        const name = a.trim();
        act(async () => {
          const r = await api.playArtistRadio(name);
          say(t("Radio « {tag} » lancée pour « {artist} » ({count} radios)", { tag: r.tag, artist: r.artist, count: r.count }));
        });
      }
    } else if (id === "__search") {
      const q = window.prompt(t("Rechercher une radio (nom) :"));
      if (q && q.trim()) setView(`search:${q.trim()}`);
    } else if (id === "__add") {
      setShowAdd((v) => !v);
    } else if (id === "local") {
      const code = localCountryCode();
      if (!code) {
        say(t("Pays indétectable depuis le navigateur — utilisez « Par pays »."));
        setView("countries");
      } else {
        setView(`local:${code}`);
      }
    } else {
      setView(id);
    }
  };

  const pinnedEntries = (): RadioEntry[] => {
    if (!list) return [];
    const all = [...list.favorites, ...list.builtin, ...list.sources];
    return pins.radios
      .map((pid) => all.find((r) => r.id === pid))
      .filter((r): r is RadioEntry => !!r);
  };

  const subView = () => {
    if (!list) return <div className="empty">{t("Chargement…")}</div>;
    if (view === "pinned") {
      const entries = pinnedEntries();
      const urls = pins.radios.filter((pid) => !entries.some((r) => r.id === pid));
      return (
        <>
          {entries.length === 0 && urls.length === 0 && <div className="empty">{t("Aucune radio épinglée — menu ⋮ d'une radio puis « Épingler ».")}</div>}
          {entries.map(row)}
          {urls.map((u) => (
            <div key={u} className="browse-row" onClick={() => act(() => api.playRadioUrl(u, u), `Lecture de « ${u} »`)} title={u}>
              <span className="row-ico"><Icon name="radio" size={20} /></span>
              <span className="name">{u}</span>
              <span className="meta">{t("épinglée")}</span>
              <button className="icon-btn" title={t("Retirer des épingles")} onClick={(ev) => { ev.stopPropagation(); onTogglePin("radio", u); }}><Icon name="pin" size={16} filled /></button>
            </div>
          ))}
        </>
      );
    }
    if (view === "fav") return list.favorites.length ? list.favorites.map(row) : <div className="empty">{t("Aucune radio favorite — ajoutez-en via « Ajouter une radio » ou le menu ⋮ d'une radio.")}</div>;
    if (view === "builtin") return list.builtin.map(row);
    if (view === "sources") return list.sources.length ? list.sources.map(row) : <div className="empty">{t("Aucune source radio configurée.")}</div>;
    // vues podcasts (#81)
    if (view === "podcasts") return podcastsView();
    if (view.startsWith("podshow:")) return podshowView();

    // ne rend les données de l'annuaire que si elles correspondent à la vue courante
    const data = dir && dir.view === view ? dir.data : null;
    if (dirLoading || !data) return <div className="empty">{t("Chargement de l'annuaire…")}</div>;
    if (view === "selection") {
      const sections = data as DirSection[];
      return (
        <>
          {sections.map((sec) => (
            <div key={sec.name}>
              <h3 style={{ margin: "14px 4px 6px", textTransform: "capitalize" }}>{sec.name}</h3>
              {sec.stations.map(stationRow)}
            </div>
          ))}
        </>
      );
    }
    if (view === "genres") return (data as DirTag[]).map(tagRow);
    if (view === "countries") return (data as DirCountry[]).map(countryRow);
    const stations = data as DirStation[];
    return stations.length ? stations.map(stationRow) : <div className="empty">{t("Aucune radio trouvée.")}</div>;
  };

  return (
    <div>
      {view !== "root" && (
        <div className="toolbar">
          <button className="icon-btn" title={t("Retour")} onClick={() => setView("root")}><Icon name="back" size={18} /></button>
          <h2 style={{ margin: 0 }}>{viewTitle()}</h2>
        </div>
      )}

      {view === "root" && (
        <>
          {!list && <div className="empty">{t("Chargement…")}</div>}
          {list && (
            <div className="grid">
              {tiles.map((t) => (
                <div key={t.id} className="card" onClick={() => onTile(t.id)}>
                  <div className="cover"><Icon name={t.icon} size={44} /></div>
                  <div className="t">{t.label}</div>
                  <div className="s">{t.sub}</div>
                </div>
              ))}
            </div>
          )}
          {showAdd && (
            <div className="toolbar" style={{ marginTop: 16 }}>
              <input type="text" placeholder={t("Nom de la radio")} value={name} onChange={(e) => setName(e.target.value)} style={{ width: 180 }}
                onKeyDown={(e) => e.key === "Enter" && submit()} />
              <input type="text" placeholder={t("URL du flux (http://… / https://…)")} value={url} onChange={(e) => setUrl(e.target.value)} style={{ width: 340 }}
                onKeyDown={(e) => e.key === "Enter" && submit()} />
              <button className="btn sm" disabled={adding || !name.trim() || !url.trim()} onClick={submit}>
                <Icon name="plus" size={14} /> {adding ? t("Ajout…") : t("Ajouter")}
              </button>
            </div>
          )}
        </>
      )}

      {view !== "root" && subView()}

      {menu && (() => {
        const tgt = menu.target;
        return (
          <>
            <div className="menu-overlay" onClick={closeMenu} onContextMenu={(e) => { e.preventDefault(); closeMenu(); }} />
            <div className="ctx-menu" style={{ left: menu.x, top: menu.y }}>
              <div className="ctx-title">{targetName(tgt)}</div>
              <div className="ctx-item" onClick={() => { playTarget(tgt); closeMenu(); }}><Icon name="play" size={16} filled /> {t("Lire maintenant")}</div>
              <div className="ctx-item" onClick={() => { act(() => api.queueAddNext(targetUrl(tgt)), " lira juste après la piste en cours"); closeMenu(); }}><Icon name="next" size={16} /> {t("Lire ensuite")}</div>
              <div className="ctx-item" onClick={() => { act(() => api.queueAdd(targetUrl(tgt)), "Ajouté à la file d'attente"); closeMenu(); }}><Icon name="plus" size={16} /> {t("Ajouter à la file d'attente")}</div>
              {!isFavUrl(targetUrl(tgt)) && (
                <div className="ctx-item" onClick={() => { addFav(tgt); closeMenu(); }}><Icon name="heart" size={16} /> {t("Ajouter aux favoris")}</div>
              )}
              {tgt.kind === "entry" && tgt.entry.origin === "favorite" && (
                <div className="ctx-item" onClick={() => { delFav(tgt.entry); closeMenu(); }}><Icon name="trash" size={16} /> {t("Supprimer des favoris")}</div>
              )}
              <div className="ctx-item" onClick={() => { onTogglePin("radio", targetPinId(tgt)); closeMenu(); }}><Icon name="pin" size={16} filled={isPinned(tgt)} /> {isPinned(tgt) ? t("Retirer des épingles") : t("Épingler")}</div>
            </div>
          </>
        );
      })()}
    </div>
  );
}

// ---------- File d'attente ----------

function Queue({ state, act, say, playAll }: { state: PlayerState | null; act: (fn: () => Promise<unknown>, ok?: string) => Promise<void>; say: (m: string) => void; playAll: (t: Track[]) => void }) {
  const queue = state?.queue ?? [];
  const idx = state?.queue_index ?? null;
  const [saving, setSaving] = useState(false);
  const save = async () => {
    const name = window.prompt("Nom de la liste de lecture :", "Ma liste");
    if (!name || !name.trim()) return;
    setSaving(true);
    try {
      await api.addPlaylist(name.trim());
      say(`Liste « ${name.trim()} » enregistrée (${queue.length} titres)`);
    } catch (e) { say(e instanceof Error ? e.message : String(e)); }
    finally { setSaving(false); }
  };
  return (
    <div>
      <div className="toolbar">
        <div className="spacer" />
        <button className="btn ghost sm" disabled={saving || queue.length === 0} onClick={save}><Icon name="download" size={16} /> Enregistrer</button>
        <button className="btn ghost sm" disabled={queue.length === 0} onClick={() => act(() => api.clearQueue(), "File vidée")}><Icon name="trash" size={16} /> Vider</button>
      </div>
      {queue.length === 0 && <div className="empty">La file d'attente est vide.</div>}
      {queue.length > 0 && (
        <table className="tracks">
          <thead><tr><th>#</th><th>Titre</th><th>Artiste</th><th>Album</th><th>Durée</th><th></th></tr></thead>
          <tbody>
            {queue.map((row, i) => (
              <tr key={`${row.id}-${i}`} className={idx === i ? "playing" : ""} onClick={() => playAll(queue.slice(i))}
                title={idx === i && state?.is_live ? t("Flux live : la radio remplace la file d'attente (1 entrée) — « Ajouter à la file » existe dans le menu ⋮ de la radio") : undefined}>
                <td className="num">{idx === i ? <Icon name="play" size={12} filled /> : i + 1}</td>
                <td>{row.title}{idx === i && state?.is_live && <span className="pill live" style={{ marginLeft: 8 }}>{t("EN DIRECT")}</span>}</td><td>{row.artist}</td><td>{row.album}</td>
                <td className="num-time">{row.duration_secs != null ? fmtTime(row.duration_secs) : "—"}</td>
                <td><button className="icon-btn" title="Retirer" onClick={(e) => { e.stopPropagation(); act(() => api.removeQueueItem(i)); }}><Icon name="close" size={16} /></button></td>
              </tr>
            ))}
          </tbody>
        </table>
      )}
    </div>
  );
}

// ---------- Listes de lecture ----------

const SMART_FIELDS: [SmartField, string][] = [
  ["title", "Titre"], ["artist", "Artiste"], ["album", "Album"], ["album_artist", "Artiste de l'album"],
  ["genre", "Genre"], ["year", "Année"], ["duration_secs", "Durée (secondes)"], ["track_number", "N° de piste"],
  ["format", "Format"], ["bit_depth", "Profondeur (bits)"], ["sample_rate", "Fréquence (Hz)"],
  ["source", "Source"], ["has_cover", "A une pochette"],
];
const SMART_OPS: [SmartOp, string][] = [
  ["eq", "est"], ["ne", "n'est pas"], ["contains", "contient"],
  ["gt", "est >"], ["lt", "est <"], ["gte", "est ≥"], ["lte", "est ≤"],
];
const SMART_NUMERIC: SmartField[] = ["year", "duration_secs", "track_number", "bit_depth", "sample_rate"];

function smartFieldLabel(f: SmartField): string {
  return SMART_FIELDS.find(([k]) => k === f)?.[1] ?? f;
}
function smartOpLabel(o: SmartOp): string {
  return SMART_OPS.find(([k]) => k === o)?.[1] ?? o;
}
/** Résumé lisible des règles d'une smart playlist (#98). */
function smartRulesSummary(r: SmartRules): string {
  const parts = r.rules.map((rule) => `${smartFieldLabel(rule.field)} ${smartOpLabel(rule.op)} ${String(rule.value)}`);
  let s = parts.join(r.match_all ? " ET " : " OU ");
  if (r.sort) s += ` · tri ${smartFieldLabel(r.sort.field)}${r.sort.desc ? " ↓" : " ↑"}`;
  if (r.limit != null) s += ` · limite ${r.limit}`;
  return s;
}

function Playlists({ act, say, playAll, playTrack, pins, onTogglePin }: { act: (fn: () => Promise<unknown>, ok?: string) => Promise<void>; say: (m: string) => void; playAll: (t: Track[]) => void; playTrack: (t: Track) => void; pins: Pins; onTogglePin: (kind: PinKind, id: string) => void }) {
  const [pls, setPls] = useState<Playlist[]>([]);
  const [detail, setDetail] = useState<PlaylistDetail | null>(null);
  const [modal, setModal] = useState<null | { mode: "new" } | { mode: "smart" } | { mode: "edit"; playlist: Playlist }>(null);
  const load = useCallback(() => { api.playlists().then(setPls).catch(() => {}); }, []);
  useEffect(() => { load(); }, [load]);
  useEffect(() => {
    if (detail) api.playlist(detail.playlist.id).then(setDetail).catch(() => {});
  }, [detail?.playlist.id]);

  if (detail) {
    return (
      <div>
        <div className="toolbar">
          <button className="btn ghost sm" onClick={() => { setDetail(null); load(); }}><Icon name="back" size={16} /> Retour</button>
          <div className="spacer" />
          <button className="btn sm" onClick={() => act(() => api.playPlaylist(detail.playlist.id))}><Icon name="play" size={14} filled /> Lire</button>
          {detail.playlist.rules && (
            <button className="btn ghost sm" onClick={() => setModal({ mode: "edit", playlist: detail.playlist })}><Icon name="edit" size={16} /> Modifier les règles</button>
          )}
          <button className="btn ghost sm" onClick={() => act(async () => { await api.deletePlaylist(detail.playlist.id); setDetail(null); load(); }, "Liste supprimée")}><Icon name="trash" size={16} /> Supprimer</button>
          <button className="btn ghost sm" onClick={async () => {
            try {
              const sh = await api.createShare("playlist", detail.playlist.id);
              const url = api.shareUrl(sh.token);
              await navigator.clipboard?.writeText(url).catch(() => {});
              say(`Lien de partage copié : ${url}`);
            } catch (e) { say(e instanceof Error ? e.message : String(e)); }
          }}><Icon name="share" size={16} /> Partager</button>
        </div>
        <h1>{detail.playlist.name} {detail.playlist.rules && <span className="badge"><Icon name="sparkles" size={11} /> SMART</span>}</h1>
        {detail.playlist.rules && <p className="dim">{smartRulesSummary(detail.playlist.rules)}</p>}
        <TrackTable tracks={detail.tracks} playTrack={playTrack} playAll={playAll} />
        {modal && <PlaylistModal mode={modal.mode} playlist={modal.mode === "edit" ? modal.playlist : null} onClose={() => setModal(null)} onSaved={() => { load(); api.playlist(detail.playlist.id).then(setDetail).catch(() => {}); }} say={say} />}
      </div>
    );
  }
  return (
    <div>
      <div className="toolbar">
        <div className="spacer" />
        <button className="btn ghost sm" onClick={() => setModal({ mode: "new" })}><Icon name="plus" size={16} /> Nouvelle liste</button>
        <button className="btn sm" onClick={() => setModal({ mode: "smart" })}><Icon name="sparkles" size={16} /> Smart playlist…</button>
      </div>
      {pls.length === 0 && <div className="empty">Aucune liste — enregistrez la file d'attente pour en créer une.</div>}
      <div className="grid">
        {[...pls].sort((a, b) => (pins.playlists.includes(b.id) ? 1 : 0) - (pins.playlists.includes(a.id) ? 1 : 0)).map((p) => (
          <div key={p.id} className="card" onClick={() => api.playlist(p.id).then(setDetail).catch((e) => say(String(e)))}>
            <div className="cover"><Icon name={p.rules ? "sparkles" : "playlist"} size={44} />
              <PinButton on={pins.playlists.includes(p.id)} onToggle={() => onTogglePin("playlist", p.id)} />
            </div>
            <div className="t">{p.name} {p.rules && <span className="badge"><Icon name="sparkles" size={10} /> SMART</span>}</div>
            <div className="s">{p.track_count ?? p.track_ids.length} titre(s)</div>
          </div>
        ))}
      </div>
      {modal && <PlaylistModal mode={modal.mode} playlist={modal.mode === "edit" ? modal.playlist : null} onClose={() => setModal(null)} onSaved={load} say={say} />}
    </div>
  );
}

/** Modale de création/édition de liste : statique ("new") ou smart (#98, "smart"/"edit"). */
function PlaylistModal({ mode, playlist, onClose, onSaved, say }: { mode: "new" | "smart" | "edit"; playlist: Playlist | null; onClose: () => void; onSaved: () => void; say: (m: string) => void }) {
  const isSmart = mode !== "new";
  const initialRules = playlist?.rules ?? null;
  const [name, setName] = useState(playlist?.name ?? "");
  const [matchAll, setMatchAll] = useState(initialRules?.match_all ?? true);
  const [rules, setRules] = useState<SmartRule[]>(initialRules?.rules ?? [{ field: "genre", op: "eq", value: "" }]);
  const [sortField, setSortField] = useState<SmartField | "">(initialRules?.sort?.field ?? "");
  const [sortDesc, setSortDesc] = useState(initialRules?.sort?.desc ?? false);
  const [limit, setLimit] = useState(initialRules?.limit != null ? String(initialRules.limit) : "");
  const [busy, setBusy] = useState(false);

  const initValue = (f: SmartField): string | number | boolean => (SMART_NUMERIC.includes(f) ? 0 : f === "has_cover" ? true : "");
  const setRule = (i: number, patch: Partial<SmartRule>) =>
    setRules((rs) => rs.map((r, j) => (j === i ? { ...r, ...patch } : r)));

  const submit = async () => {
    if (!name.trim()) { say(t("Nom requis")); return; }
    setBusy(true);
    try {
      if (!isSmart) {
        if (playlist) await api.updatePlaylist(playlist.id, { name: name.trim(), rules: null });
        else await api.addPlaylist(name.trim());
        say(playlist ? t("Liste renommée") : t("Liste créée"));
      } else {
        const clean = rules
          .map((r) => ({ ...r, value: SMART_NUMERIC.includes(r.field) && typeof r.value === "string" && r.value.trim() !== "" ? Number(r.value) : r.value }))
          .filter((r) => !(typeof r.value === "string" && r.value.trim() === ""));
        if (clean.length === 0) { say(t("Ajoutez au moins une règle")); setBusy(false); return; }
        const sr: SmartRules = {
          match_all: matchAll,
          rules: clean,
          sort: sortField ? { field: sortField, desc: sortDesc } : null,
          limit: limit.trim() ? Math.max(1, parseInt(limit, 10) || 1) : null,
        };
        if (playlist) await api.updatePlaylist(playlist.id, { name: name.trim(), rules: sr });
        else await api.addPlaylist(name.trim(), undefined, undefined, sr);
        say(playlist ? t("Smart playlist modifiée") : t("Smart playlist créée"));
      }
      onSaved();
      onClose();
    } catch (e) { say(e instanceof Error ? e.message : String(e)); }
    finally { setBusy(false); }
  };

  return (
    <div className="modal-bg" onClick={onClose}>
      <div className="modal wide" onClick={(e) => e.stopPropagation()}>
        <div className="modal-head">
          <h2>{mode === "edit" ? t("Modifier la smart playlist") : isSmart ? t("Nouvelle smart playlist") : t("Nouvelle liste")}</h2>
          <button className="icon-btn" onClick={onClose}><Icon name="close" size={20} /></button>
        </div>
        <label>{t("Nom")}
          <input type="text" value={name} onChange={(e) => setName(e.target.value)} placeholder={t("Ma smart playlist")} autoFocus />
        </label>
        {isSmart && (
          <>
            <div className="smart-rules-head">
              <span className="dim">{t("Correspondre à")}</span>
              <select value={matchAll ? "all" : "any"} onChange={(e) => setMatchAll(e.target.value === "all")}>
                <option value="all">{t("toutes les règles (ET)")}</option>
                <option value="any">{t("au moins une règle (OU)")}</option>
              </select>
            </div>
            {rules.map((r, i) => (
              <div className="smart-rule-row" key={i}>
                <select value={r.field} onChange={(e) => setRule(i, { field: e.target.value as SmartField, value: initValue(e.target.value as SmartField) })}>
                  {SMART_FIELDS.map(([k, label]) => <option key={k} value={k}>{t(label)}</option>)}
                </select>
                <select value={r.op} onChange={(e) => setRule(i, { op: e.target.value as SmartOp })}>
                  {SMART_OPS.map(([k, label]) => <option key={k} value={k}>{t(label)}</option>)}
                </select>
                {r.field === "has_cover" ? (
                  <select value={String(r.value)} onChange={(e) => setRule(i, { value: e.target.value === "true" })}>
                    <option value="true">{t("oui")}</option>
                    <option value="false">{t("non")}</option>
                  </select>
                ) : (
                  <input type="text" value={String(r.value)} inputMode={SMART_NUMERIC.includes(r.field) ? "numeric" : undefined}
                    placeholder={SMART_NUMERIC.includes(r.field) ? "0" : t("valeur…")}
                    onChange={(e) => setRule(i, { value: e.target.value })} />
                )}
                <button className="icon-btn danger" title={t("Retirer la règle")} onClick={() => setRules((rs) => rs.filter((_, j) => j !== i))}><Icon name="minus" size={16} /></button>
              </div>
            ))}
            <button className="btn ghost sm" onClick={() => setRules((rs) => [...rs, { field: "genre", op: "eq", value: "" }])}><Icon name="plus" size={14} /> {t("Ajouter une règle")}</button>
            <div className="smart-rule-row">
              <select value={sortField} onChange={(e) => setSortField(e.target.value as SmartField | "")}>
                <option value="">{t("Trier par… (défaut : bibliothèque)")}</option>
                {SMART_FIELDS.map(([k, label]) => <option key={k} value={k}>{t(label)}</option>)}
              </select>
              <select value={sortDesc ? "desc" : "asc"} onChange={(e) => setSortDesc(e.target.value === "desc")} disabled={!sortField}>
                <option value="asc">{t("croissant")}</option>
                <option value="desc">{t("décroissant")}</option>
              </select>
              <input type="text" value={limit} inputMode="numeric" placeholder={t("Limite (vide = tout)")} onChange={(e) => setLimit(e.target.value)} />
            </div>
          </>
        )}
        <div className="modal-foot">
          <button className="btn ghost" onClick={onClose}>{t("Annuler")}</button>
          <button className="btn" disabled={busy || !name.trim()} onClick={submit}>{busy ? "…" : isSmart ? t("Enregistrer") : t("Créer")}</button>
        </div>
      </div>
    </div>
  );
}

// ---------- Sources & réglages ----------

function Switch({ checked, onChange, label }: { checked: boolean; onChange: (v: boolean) => void; label?: string }) {
  return (
    <button className={`switch ${checked ? "on" : ""}`} role="switch" aria-checked={checked} aria-label={label}
      onClick={(e) => { e.stopPropagation(); onChange(!checked); }}>
      <span className="knob" />
    </button>
  );
}

function Sources({ sources, refresh, say }: { sources: Source[]; refresh: () => Promise<void>; say: (m: string) => void }) {
  const [modal, setModal] = useState<null | { mode: "add" } | { mode: "edit"; source: Source }>(null);

  const srcSummary = (s: Source): string => {
    const r = s as Record<string, unknown>;
    switch (s.kind) {
      case "local": case "usb": return String(r.path ?? "");
      case "smb": return `smb://${r.host}/${r.share}${r.base_path ? `/${r.base_path}` : ""}`;
      case "radio": case "http": return String(r.url ?? r.base_url ?? "");
      case "upnp": return String(r.url ?? "");
      case "qobuz": return `${String(r.user ?? "")} · ${String(r.quality ?? "hires")}`;
      case "tidal": return `session · ${String(r.quality ?? "lossless")}`;
      case "linein": return r.device ? String(r.device) : "entrée par défaut";
      default: return "";
    }
  };

  return (
    <div>
      <div className="toolbar">
        <div className="spacer" />
        <button className="btn" onClick={() => setModal({ mode: "add" })}><Icon name="plus" size={16} /> Ajouter une source</button>
      </div>

      <h2>{t("Vos sources")}</h2>
      {sources.length === 0 && <div className="empty">{t("Aucune source configurée.")}</div>}
      {sources.map((s) => (
        <div key={s.id} className="src-card">
          <span className="src-ico"><Icon name={kindIconName(s.kind)} size={24} /></span>
          <span className="src-main">
            <span className="src-name">{s.name} <span className="badge">{kindLabel(s.kind)}</span></span>
            <span className="src-sum">{srcSummary(s)}</span>
          </span>
          <span className={`pill ${s.enabled ? "ok" : ""}`}>{s.enabled ? "Active" : "Désactivée"}</span>
          <Switch checked={s.enabled} label={`activer ${s.name}`}
            onChange={async (v) => { try { await api.toggleSource(s.id, v); await refresh(); } catch (e) { say(String(e)); } }} />
          <button className="icon-btn" title="Modifier" onClick={() => setModal({ mode: "edit", source: s })}><Icon name="edit" size={18} /></button>
          <button className="icon-btn danger" title="Supprimer" onClick={async () => { try { await api.deleteSource(s.id); say(`Source « ${s.name} » supprimée`); await refresh(); } catch (e) { say(String(e)); } }}><Icon name="trash" size={18} /></button>
        </div>
      ))}

      {modal && <SourceModal source={modal.mode === "edit" ? modal.source : null} sources={sources} onClose={() => setModal(null)} refresh={refresh} say={say} />}
    </div>
  );
}

function SourceModal({ source, sources, onClose, refresh, say }: { source: Source | null; sources: Source[]; onClose: () => void; refresh: () => Promise<void>; say: (m: string) => void }) {
  const editing = source;
  const [tab, setTab] = useState<SourceKindTab>(editing ? (editing.kind as SourceKindTab) : "local");
  const [name, setName] = useState(editing?.name ?? "");
  const [path, setPath] = useState(String((editing as Record<string, unknown> | null)?.path ?? ""));
  const [host, setHost] = useState(String((editing as Record<string, unknown> | null)?.host ?? ""));
  const [share, setShare] = useState(String((editing as Record<string, unknown> | null)?.share ?? ""));
  const [user, setUser] = useState(String((editing as Record<string, unknown> | null)?.user ?? ""));
  const [password, setPassword] = useState("");
  const [basePath, setBasePath] = useState(String((editing as Record<string, unknown> | null)?.base_path ?? ""));
  const [url, setUrl] = useState(String((editing as Record<string, unknown> | null)?.url ?? (editing as Record<string, unknown> | null)?.base_url ?? ""));
  const [quality, setQuality] = useState(String((editing as Record<string, unknown> | null)?.quality ?? "hires"));
  const [token, setToken] = useState("");
  const [clientId, setClientId] = useState(String((editing as Record<string, unknown> | null)?.client_id ?? ""));
  const [sessionId, setSessionId] = useState(String((editing as Record<string, unknown> | null)?.session_id ?? ""));
  const [mounts, setMounts] = useState<MountInfo[]>([]);
  const [detecting, setDetecting] = useState(false);
  const [inDevice, setInDevice] = useState(String((editing as Record<string, unknown> | null)?.device ?? ""));
  const [inDevices, setInDevices] = useState<InputDevice[]>([]);
  const [busy, setBusy] = useState(false);

  const uniqueId = (base: string) => {
    let id = base;
    let n = 2;
    while (sources.some((s) => s.id === id && s.id !== editing?.id)) { id = `${base}-${n}`; n += 1; }
    return id;
  };

  const build = (): Source | null => {
    if (!name.trim()) return null;
    const id = editing?.id ?? uniqueId(tab === "usb" ? `usb-${slugify(name)}` : slugify(name));
    switch (tab) {
      case "local": return path.trim() ? makeSource(id, name.trim(), "local", { path: path.trim() }) : null;
      case "usb": return path.trim() ? makeSource(id, name.trim(), "usb", { path: path.trim() }) : null;
      case "smb": return host.trim() && share.trim() ? makeSource(id, name.trim(), "smb", { host: host.trim(), share: share.trim(), user: user.trim(), password, base_path: basePath.trim() }) : null;
      case "radio": return url.trim() ? makeSource(id, name.trim(), "radio", { url: url.trim() }) : null;
      case "http": return makeSource(id, name.trim(), "http", { base_url: url.trim() || null });
      case "upnp": return url.trim() ? makeSource(id, name.trim(), "upnp", { url: url.trim() }) : null;
      case "qobuz": return user.trim() && password ? makeSource(id, name.trim(), "qobuz", { user: user.trim(), password, quality }) : null;
      case "tidal": return token.trim() ? makeSource(id, name.trim(), "tidal", { token: token.trim(), session_id: sessionId.trim() || null, quality: quality === "hires" ? "hires" : "lossless" }) : null;
      case "linein": return makeSource(id, name.trim(), "linein", { device: inDevice || null });
      case "archive": return makeSource(id, name.trim(), "archive", {});
      case "deezer": return makeSource(id, name.trim(), "deezer", {});
      case "jamendo": return makeSource(id, name.trim(), "jamendo", { client_id: clientId.trim() || null });
      case "youtube": return makeSource(id, name.trim(), "youtube", {});
    }
  };

  const submit = async () => {
    const src = build();
    if (!src) { say("Champs obligatoires manquants"); return; }
    setBusy(true);
    try {
      if (editing) await api.updateSource(editing.id, src);
      else await api.addSource(src);
      say(editing ? `Source « ${src.name} » modifiée` : `Source « ${src.name} » ajoutée`);
      onClose();
      await refresh();
    } catch (e) { say(e instanceof Error ? e.message : String(e)); }
    finally { setBusy(false); }
  };

  const detect = async () => {
    setDetecting(true);
    try { setMounts(await api.usbDevices()); }
    catch (e) { say(e instanceof Error ? e.message : String(e)); }
    finally { setDetecting(false); }
  };

  const kinds: [SourceKindTab, string][] = [["local", "Dossier local"], ["usb", "USB"], ["smb", "NAS (SMB/CIFS)"], ["radio", "Radio"], ["http", "URL HTTP"], ["upnp", "UPnP/DLNA"], ["qobuz", "Qobuz"], ["tidal", "Tidal"], ["linein", "Line-in"], ["archive", "Internet Archive"], ["deezer", "Deezer (30 s)"], ["jamendo", "Jamendo (CC)"], ["youtube", "YouTube (yt-dlp)"]];

  useEffect(() => {
    if (tab === "linein") api.inputDevices().then(setInDevices).catch(() => {});
  }, [tab]);

  return (
    <div className="modal-bg" onClick={onClose}>
      <div className="modal wide" onClick={(e) => e.stopPropagation()}>
        <div className="modal-head">
          <h2>{editing ? `Modifier « ${editing.name} »` : "Ajouter une source"}</h2>
          <button className="icon-btn" onClick={onClose}><Icon name="close" size={20} /></button>
        </div>

        <div className="seg">
          {kinds.map(([k, label]) => (
            <button key={k} className={`seg-btn ${tab === k ? "active" : ""}`} disabled={!!editing && editing.kind !== k} onClick={() => setTab(k)}>{label}</button>
          ))}
        </div>

        <div className="form-grid">
          <label>Nom / alias<input type="text" value={name} onChange={(e) => setName(e.target.value)} placeholder={tab === "smb" ? "musique" : "Ma musique"} /></label>
          {(tab === "local" || tab === "usb") && (
            <label className="span2">Chemin du dossier<input type="text" value={path} onChange={(e) => setPath(e.target.value)} placeholder="C:\Musique · /mnt/nas/musique · E:\" /></label>
          )}
          {tab === "usb" && (
            <div className="span2">
              <button className="btn ghost sm" disabled={detecting} onClick={detect}><Icon name="usb" size={16} /> {detecting ? "Détection…" : "Détecter les disques USB"}</button>
              {mounts.length > 0 && (
                <div className="mounts">
                  {mounts.map((m) => (
                    <button key={m.path} className="mount-row" onClick={() => { setPath(m.path); setName((n) => n || m.label); }}>
                      <Icon name="usb" size={18} /> {m.label} <span className="meta">{m.path}</span>
                    </button>
                  ))}
                </div>
              )}
            </div>
          )}
          {tab === "smb" && (
            <>
              <label>Adresse IP du NAS<input type="text" value={host} onChange={(e) => setHost(e.target.value)} placeholder="192.168.1.10" /></label>
              <label>Type de partage
                <select value="CIFS" disabled><option>CIFS (SMB)</option></select>
              </label>
              <label>Chemin (partage)<input type="text" value={share} onChange={(e) => setShare(e.target.value)} placeholder="music" /></label>
              <label>Nom d'utilisateur (facultatif)<input type="text" value={user} onChange={(e) => setUser(e.target.value)} placeholder="laisser vide = accès invité (guest)" /></label>
              <label>Mot de passe (facultatif)<input type="password" value={password} onChange={(e) => setPassword(e.target.value)} placeholder="••••••" /></label>
              <details className="span2 adv">
                <summary><Icon name="chevron" size={16} /> Options avancées</summary>
                <label>Sous-dossier racine (facultatif)<input type="text" value={basePath} onChange={(e) => setBasePath(e.target.value)} placeholder="flac" /></label>
              </details>
            </>
          )}
          {(tab === "radio" || tab === "http") && (
            <label className="span2">URL{tab === "http" ? " (m3u/m3u8 ou fichier direct, facultatif)" : " du flux"}<input type="text" value={url} onChange={(e) => setUrl(e.target.value)} placeholder="https://…" /></label>
          )}
          {tab === "archive" && (
            <div className="span2 meta">Catalogue audio gratuit d'archive.org (Internet Archive) — recherche, albums et lecture complète, sans compte ni clé.</div>
          )}
          {tab === "deezer" && (
            <div className="span2 meta">Extraits 30 s via l'API publique Deezer — recherche et top du chart jouables directement, sans compte ni clé.</div>
          )}
          {tab === "jamendo" && (
            <>
              <div className="span2 meta">Musique Creative Commons (Jamendo) — lecture complète, sans compte. Inscrivez-vous gratuitement sur developer.jamendo.com pour obtenir un client_id.</div>
              <label className="span2">client_id Jamendo<input type="text" value={clientId} onChange={(e) => setClientId(e.target.value)} placeholder="votre client_id (developer.jamendo.com)" /></label>
            </>
          )}
          {tab === "youtube" && (
            <div className="span2 meta">Recherche YouTube et lecture audio (bestaudio) via le binaire <code>yt-dlp</code> (env <code>AURIS_YTDLP</code> ou PATH) — contenu public sans compte Google.</div>
          )}
          {tab === "upnp" && (
            <label className="span2">URL de description du serveur UPnP/DLNA (ex. http://192.168.1.10:8200/desc.xml) — utile si la découverte SSDP est bloquée<input type="text" value={url} onChange={(e) => setUrl(e.target.value)} placeholder="http://…/desc.xml" /></label>
          )}
          {tab === "linein" && (
            <>
              <label className="span2">Périphérique d'entrée (line-in, optique, HDMI eARC, boucle « ce que tu entends »…)
                <select value={inDevice} onChange={(e) => setInDevice(e.target.value)}>
                  <option value="">Entrée par défaut{` ★`}</option>
                  {inDevices.map((d) => <option key={d.name} value={d.name}>{d.name}{d.is_default ? " ★" : ""}</option>)}
                </select>
              </label>
              <div className="span2 meta">La source joue l'entrée en direct (badge « EN DIRECT »), en passthrough vers la sortie choisie. Sous Windows, utilisez une boucle (ex. « Stereo Mix » / VB-Cable) pour capturer la sortie du PC.</div>
            </>
          )}
          {tab === "qobuz" && (
            <>
              <label>Identifiant Qobuz (e-mail)<input type="text" value={user} onChange={(e) => setUser(e.target.value)} placeholder="prenom@exemple.com" /></label>
              <label>Mot de passe<input type="password" value={password} onChange={(e) => setPassword(e.target.value)} placeholder="••••••" autoComplete="new-password" /></label>
              <label>Qualité de lecture
                <select value={quality} onChange={(e) => setQuality(e.target.value)}>
                  <option value="hires">Hi-Res (jusqu'à 24 bits / 192 kHz)</option>
                  <option value="flac">FLAC 16 bits / 44,1 kHz</option>
                  <option value="mp3">MP3 320 kbps</option>
                </select>
              </label>
              <div className="span2 meta">La lecture passe par l'API Qobuz avec votre compte ; les pistes sont résolues en URL signée à la lecture. La navigation (favoris, playlists, recherche) est disponible dans « Parcourir ».</div>
            </>
          )}
          {tab === "tidal" && (
            <>
              <label className="span2">Token bearer de session (JWT — inspecteur réseau du lecteur web Tidal, requête `api.tidal.com`, en-tête `Authorization: Bearer …`)<input type="password" value={token} onChange={(e) => setToken(e.target.value)} placeholder="eyJ…" autoComplete="new-password" /></label>
              <label>Session id (cookie `sid` du navigateur, facultatif si absent du token)<input type="text" value={sessionId} onChange={(e) => setSessionId(e.target.value)} placeholder="xxxxxxxx-xxxx-…" /></label>
              <label>Qualité de lecture
                <select value={quality} onChange={(e) => setQuality(e.target.value)}>
                  <option value="lossless">Lossless (16 bits / 44,1 kHz)</option>
                  <option value="hires">Hi-Res lossless (FLAC 24 bits, MQA)</option>
                </select>
              </label>
              <div className="span2 meta">La lecture passe par l'API v1 Tidal (`playbackinfopostpaywall`, manifest non chiffré) ; les pistes sont résolues en URL de flux à la lecture. Favoris, playlists et recherche dans « Parcourir ».</div>
            </>
          )}
        </div>

        <div className="modal-foot">
          <button className="btn ghost" onClick={onClose}>Annuler</button>
          <button className="btn" disabled={busy || !build()} onClick={submit}>{busy ? "…" : editing ? "Enregistrer les modifications" : "Ajouter la source"}</button>
        </div>
      </div>
    </div>
  );
}

// ---------- Réglages (menu façon Volumio Nova) ----------

function SetRow({ label, desc, children }: { label: string; desc?: string; children: React.ReactNode }) {
  return (
    <div className="set-row">
      <div className="set-main">
        <div className="set-label">{label}</div>
        {desc && <div className="set-desc">{desc}</div>}
      </div>
      <div className="set-ctrl">{children}</div>
    </div>
  );
}

/// Ligne de section façon Nova (#92) : icône + titre + chevron, le corps se déplie au clic.
/// En recherche (q non vide), `open` est forcé à true côté appelant.
function SetGroup({ icon, title, open, onToggle, children }: { icon: string; title: string; open: boolean; onToggle: () => void; children: React.ReactNode }) {
  return (
    <div className={`set-group ${open ? "open" : ""}`}>
      <button type="button" className="set-group-head" onClick={onToggle}>
        <span className="set-group-ico"><Icon name={icon} size={20} /></span>
        <span className="set-group-title">{title}</span>
        <Icon name="chevron" size={18} className="set-group-chev" />
      </button>
      {open && <div className="set-group-body">{children}</div>}
    </div>
  );
}

/// Interfaces réseau : état + activer/désactiver (#44).
function NetInterfaces({ say }: { say: (m: string) => void }) {
  const [ifs, setIfs] = useState<NetInterface[] | null>(null);
  const [busy, setBusy] = useState(false);
  const load = useCallback(async () => {
    try { setIfs(await api.netInterfaces()); } catch (e) { say(e instanceof Error ? e.message : String(e)); }
  }, [say]);
  useEffect(() => { load(); }, [load]);
  const toggle = async (name: string, up: boolean) => {
    setBusy(true);
    try {
      setIfs(await api.netInterfaceAction(name, up ? "down" : "up"));
      say(up ? `${name} désactivée` : `${name} activée`);
    } catch (e) { say(e instanceof Error ? e.message : String(e)); }
    finally { setBusy(false); }
  };
  if (!ifs) return <div className="empty">{t("Chargement…")}</div>;
  return (
    <div style={{ display: "flex", flexDirection: "column", gap: 6, minWidth: 440 }}>
      {ifs.map((i) => (
        <div key={i.name} className="mr-peer" style={{ justifyContent: "space-between" }}>
          <div style={{ minWidth: 0 }}>
            <div className="mr-name">
              {i.name} {i.wifi && <span className="badge">Wi-Fi</span>} {i.loopback && <span className="badge">loopback</span>} {i.default_route && <span className="badge">{t("route par défaut")}</span>}
            </div>
            <div className="mr-base">{i.ips.join(", ") || t("pas d'adresse IP")}{i.mac ? ` · ${i.mac}` : ""}</div>
          </div>
          <div style={{ display: "flex", gap: 6, alignItems: "center", flex: "none" }}>
            <span className={`badge ${i.up ? "live" : ""}`}>{i.up ? t("active") : t("inactive")}</span>
            {!i.loopback && (
              <button className="btn ghost sm" disabled={busy} onClick={() => toggle(i.name, i.up)}>
                {i.up ? t("Désactiver") : t("Activer")}
              </button>
            )}
          </div>
        </div>
      ))}
      <button className="btn ghost sm" style={{ alignSelf: "flex-start" }} onClick={load} disabled={busy}>
        <Icon name="refresh" size={16} /> {t("Actualiser")}
      </button>
    </div>
  );
}

function SettingsView({ settings, updateSettings, say, refresh, packs, searchQ }: {
  settings: Settings | null;
  updateSettings: (patch: (s: Settings) => Settings, ok?: string) => Promise<void>;
  say: (m: string) => void;
  refresh: () => Promise<void>;
  packs: Packs | null;
  searchQ: string;
}) {
  // La recherche des réglages passe par la barre unique de la topbar (#99).
  const q = searchQ;
  // Section dépliée de la liste façon Nova (null = toutes repliées).
  const [openSec, setOpenSec] = useState<string | null>(t("Lecture"));
  const [storage, setStorage] = useState<StorageInfo | null>(null);
  const [logs, setLogs] = useState<string | null>(null);
  const [busy, setBusy] = useState("");
  const [shares, setShares] = useState<ShareLink[]>([]);
  const loadShares = useCallback(() => { api.listShares().then(setShares).catch(() => {}); }, []);
  useEffect(() => { loadShares(); }, [loadShares]);

  // Édition de packs communautaires (thème CSS / pack de langue) — #41.
  const [packKind, setPackKind] = useState<"theme" | "locale">("theme");
  const [packId, setPackId] = useState("");
  const [packBody, setPackBody] = useState("");
  const [packBusy, setPackBusy] = useState(false);
  const savePack = async () => {
    const id = packId.trim();
    if (!id || !packBody.trim() || packBusy) return;
    setPackBusy(true);
    try {
      if (packKind === "theme") await api.saveThemePack(id, packBody);
      else await api.saveLocalePack(id, packBody);
      say(t("Pack « {id} » enregistré", { id }));
      setPackId(""); setPackBody("");
      await refresh();
    } catch (e) { say(e instanceof Error ? e.message : String(e)); }
    finally { setPackBusy(false); }
  };
  const delPack = async (kind: "theme" | "locale", id: string) => {
    try {
      if (kind === "theme") await api.deleteThemePack(id);
      else await api.deleteLocalePack(id);
      say(t("Pack « {id} » supprimé", { id }));
      await refresh();
    } catch (e) { say(e instanceof Error ? e.message : String(e)); }
  };

  // Statut DoP/WASAPI des périphériques (Windows uniquement).
  const [dop, setDop] = useState<DopStatus | null>(null);
  const [dopRate, setDopRate] = useState(176400);
  const [dopBusy, setDopBusy] = useState(false);
  const loadDop = useCallback(() => { api.dopStatus().then(setDop).catch(() => {}); }, []);
  useEffect(() => { loadDop(); }, [loadDop]);

  // Sauvegarde / restauration (#40)
  const [davBase, setDavBase] = useState("");
  const [davFile, setDavFile] = useState("");
  const [davUser, setDavUser] = useState("");
  const [davPass, setDavPass] = useState("");
  const fileRef = useRef<HTMLInputElement | null>(null);
  // jeton d'autorisation Last.fm en cours de validation (#26)
  const lfToken = useRef<string | null>(null);

  useEffect(() => {
    api.systemStorage().then(setStorage).catch(() => {});
  }, []);

  if (!settings) return <div className="empty">{t("Chargement des réglages…")}</div>;

  const qn = q.trim().toLowerCase();
  const show = (label: string, desc?: string) =>
    !qn || label.toLowerCase().includes(qn) || (desc ?? "").toLowerCase().includes(qn);

  const setPlayback = <K extends keyof Settings["playback"]>(k: K, v: Settings["playback"][K]) =>
    updateSettings((s) => ({ ...s, playback: { ...s.playback, [k]: v } }));
  const setAppearance = <K extends keyof Settings["appearance"]>(k: K, v: Settings["appearance"][K]) =>
    updateSettings((s) => ({ ...s, appearance: { ...s.appearance, [k]: v } }));
  const setSystem = <K extends keyof Settings["system"]>(k: K, v: Settings["system"][K]) =>
    updateSettings((s) => ({ ...s, system: { ...s.system, [k]: v } }));
  const setNetwork = <K extends keyof Settings["network"]>(k: K, v: Settings["network"][K]) =>
    updateSettings((s) => ({ ...s, network: { ...s.network, [k]: v } }));
  const setSources = <K extends keyof Settings["sources"]>(k: K, v: Settings["sources"][K]) =>
    updateSettings((s) => ({ ...s, sources: { ...s.sources, [k]: v } }));
  const setLastfm = <K extends keyof Settings["lastfm"]>(k: K, v: Settings["lastfm"][K]) =>
    updateSettings((s) => ({ ...s, lastfm: { ...s.lastfm, [k]: v } }));
  const setMb = <K extends keyof Settings["musicbrainz"]>(k: K, v: Settings["musicbrainz"][K]) =>
    updateSettings((s) => ({ ...s, musicbrainz: { ...s.musicbrainz, [k]: v } }));

  const power = async (fn: () => Promise<unknown>, label: string) => {
    setBusy(label);
    try { await fn(); say(`${label} demandé`); }
    catch (e) { say(e instanceof Error ? e.message : String(e)); }
    finally { setBusy(""); }
  };

  const reset = async () => {
    if (!window.confirm("Reset usine : bibliothèque, playlists, sources et réglages seront remis à zéro. Continuer ?")) return;
    setBusy("reset");
    try {
      await api.systemReset();
      say("Reset effectué — redémarrage recommandé");
      await refresh();
    } catch (e) { say(e instanceof Error ? e.message : String(e)); }
    finally { setBusy(""); }
  };

  const downloadBackup = async () => {
    setBusy("backup");
    try {
      const blob = await api.backup();
      const a = document.createElement("a");
      a.href = URL.createObjectURL(blob);
      a.download = `auris-backup-${new Date().toISOString().replace(/[:T]/g, "-").slice(0, 19)}.zip`;
      a.click();
      URL.revokeObjectURL(a.href);
      say("Sauvegarde téléchargée");
    } catch (e) { say(e instanceof Error ? e.message : String(e)); }
    finally { setBusy(""); }
  };

  const restoreFile = async (f: File) => {
    if (!window.confirm(`Restaurer la sauvegarde « ${f.name} » ? Réglages, favoris, playlists, historique et partages actuels seront remplacés.`)) return;
    setBusy("restore");
    try {
      const r = await api.restore(f);
      say(`Sauvegarde restaurée (${r.files} fichiers)`);
      setTimeout(() => window.location.reload(), 800);
    } catch (e) { say(e instanceof Error ? e.message : String(e)); }
    finally { setBusy(""); }
  };

  const pushWebdav = async () => {
    setBusy("dav-push");
    try {
      const r = await api.backupWebdav({ url: davBase.trim(), user: davUser.trim() || undefined, password: davPass || undefined });
      say(`Sauvegarde envoyée : ${r.pushed} (${fmtBytes(r.bytes)})`);
    } catch (e) { say(e instanceof Error ? e.message : String(e)); }
    finally { setBusy(""); }
  };

  const restoreWebdav = async () => {
    if (!window.confirm("Restaurer la sauvegarde depuis WebDAV ? Les données actuelles seront remplacées.")) return;
    setBusy("dav-restore");
    try {
      const r = await api.restoreWebdav({ url: davFile.trim(), user: davUser.trim() || undefined, password: davPass || undefined });
      say(`Sauvegarde restaurée depuis WebDAV (${r.files} fichiers)`);
      setTimeout(() => window.location.reload(), 800);
    } catch (e) { say(e instanceof Error ? e.message : String(e)); }
    finally { setBusy(""); }
  };

  const p = settings.playback, ap = settings.appearance, sy = settings.system, nw = settings.network, so = settings.sources;
  const lf = settings.lastfm, mb = settings.musicbrainz;

  const sections: { icon: string; title: string; rows: React.ReactNode[] }[] = [
    {
      icon: "play",
      title: t("Lecture"),
      rows: [
        show("Courbe de volume", "Logarithmique (naturelle) ou linéaire") && (
          <SetRow key="curve" label="Courbe de volume" desc="Logarithmique (naturelle) ou linéaire">
            <select value={p.volume_curve} onChange={(e) => setPlayback("volume_curve", e.target.value as Settings["playback"]["volume_curve"])}>
              <option value="logarithmic">Logarithmique (naturelle)</option>
              <option value="linear">Linéaire</option>
            </select>
          </SetRow>
        ),
        show("Volume maximal", "Le volume courant ne dépassera jamais cette valeur") && (
          <SetRow key="maxvol" label="Volume maximal" desc={`Actuellement ${p.max_volume} / 100`}>
            <div className="range-ctrl">
              <input type="range" min={1} max={100} value={p.max_volume} aria-label="Volume maximal"
                onChange={(e) => setPlayback("max_volume", Number(e.target.value))} />
              <span className="vol">{p.max_volume}</span>
            </div>
          </SetRow>
        ),
        show("Mode de volume", "Logiciel (dans Auris) ou désactivé = bit-perfect, volume sur le DAC/ampli") && (
          <SetRow key="volmode" label="Mode de volume" desc="Désactivé = bit-perfect : aucun traitement, gain 1.0, volume réglé sur le DAC/ampli">
            <select value={p.volume_mode} onChange={(e) => setPlayback("volume_mode", e.target.value as Settings["playback"]["volume_mode"])}>
              <option value="software">Logiciel (dans Auris)</option>
              <option value="disabled">Désactivé (bit-perfect)</option>
            </select>
          </SetRow>
        ),
        show("Lecture gapless", "Enchaînement sans trou entre pistes de même format") && (
          <SetRow key="gapless" label="Lecture gapless" desc="Enchaînement sans trou entre pistes de même format">
            <Switch checked={p.gapless} label="gapless" onChange={(v) => setPlayback("gapless", v)} />
          </SetRow>
        ),
        show("Fondu enchaîné", "Durée du crossfade entre deux pistes") && (
          <SetRow key="crossfade" label="Fondu enchaîné (crossfade)" desc="Superposition de la fin et du début des pistes (même format uniquement)">
            <div className="range-ctrl">
              <input type="range" min={0} max={12} step={0.5} value={p.crossfade_secs} aria-label="Fondu enchaîné"
                onChange={(e) => setPlayback("crossfade_secs", Number(e.target.value))} />
              <span className="vol">{p.crossfade_secs}s</span>
            </div>
          </SetRow>
        ),
        show("Normalisation du volume", "Niveau sonore homogène entre les pistes (loudness R128)") && (
          <SetRow key="loudness" label="Normalisation du volume (R128)" desc="Gain statique par piste (LUFS intégré) pour un niveau homogène — lancez l'analyse de la bibliothèque">
            <div style={{ display: "flex", gap: 8, alignItems: "center", flexWrap: "wrap" }}>
              <select value={p.loudness_target_lufs} aria-label="Cible LUFS"
                onChange={(e) => setPlayback("loudness_target_lufs", Number(e.target.value))}>
                <option value={-14}>-14 LUFS (streaming)</option>
                <option value={-16}>-16 LUFS</option>
                <option value={-23}>-23 LUFS (EBU R128)</option>
              </select>
              <Switch checked={p.loudness} label="loudness" onChange={(v) => setPlayback("loudness", v)} />
              <button className="btn ghost sm" onClick={() => api.loudnessScan()
                .then((r) => say(`${r.total} pistes à analyser (${r.cached} déjà en cache)`))
                .catch((e) => say(e instanceof Error ? e.message : String(e)))}>
                {t("Analyser la bibliothèque")}
              </button>
            </div>
          </SetRow>
        ),
        show("Rééchantillonnage", "Autoriser la conversion de fréquence d'échantillonnage") && (
          <SetRow key="resampling" label="Rééchantillonnage" desc="Désactivé = bit-perfect, aucun resample (recommandé)">
            <Switch checked={p.resampling} label="rééchantillonnage" onChange={(v) => setPlayback("resampling", v)} />
          </SetRow>
        ),
        show("Lecture DSD", "DoP vers un DAC compatible, ou conversion en PCM") && (
          <SetRow key="dsd" label="Lecture DSD (DSF/DFF)" desc="DoP vers un DAC compatible, ou conversion en PCM haute fréquence">
            <select value={p.dsd_mode} onChange={(e) => setPlayback("dsd_mode", e.target.value as Settings["playback"]["dsd_mode"])}>
              <option value="dop">DSD over PCM (DoP)</option>
              <option value="pcm">Converti en PCM</option>
            </select>
          </SetRow>
        ),
        show("Minuterie de sommeil", "Pause automatique après N minutes de lecture (one-shot)") && (
          <SetRow key="sleep" label="Minuterie de sommeil" desc="Pause automatique après N minutes (one-shot, réarmable)">
            <select value={p.sleep_timer?.toString() ?? ""} onChange={(e) => setPlayback("sleep_timer", e.target.value === "" ? null : Number(e.target.value))}>
              <option value="">Désactivée</option>
              {[15, 30, 45, 60, 90, 120].map((m) => <option key={m} value={m}>{m} min</option>)}
            </select>
          </SetRow>
        ),
        show("Réveil", "Relance la lecture à une heure donnée (heure locale)") && (
          <SetRow key="wake" label="Réveil" desc="Relance la lecture à l'heure indiquée (heure locale)">
            <input type="time" value={p.wake_time ?? ""} style={{ width: 130 }}
              onChange={(e) => setPlayback("wake_time", e.target.value || null)} />
          </SetRow>
        ),
      ].filter(Boolean) as React.ReactNode[],
    },
    {
      icon: "sliders",
      title: t("Égaliseur"),
      rows: [
        show("Égaliseur paramétrique", "10 bandes, ±20 dB, appliqué en temps réel (DAC et « Jouer ici »)") && (
          <SetRow key="eq" label="Égaliseur paramétrique" desc="10 bandes ISO (31 Hz – 16 kHz), ±20 dB, appliqué à chaud sur le flux PCM">
            <Switch checked={p.eq.enabled} label="égaliseur" onChange={(v) => setPlayback("eq", { ...p.eq, enabled: v })} />
          </SetRow>
        ),
        p.eq.enabled && show("Bandes de l'égaliseur", "Gain de chaque bande en dB") && (
          <div key="eqbands" className="eq-grid">
            {p.eq.bands.map((b, i) => (
              <div key={b.freq_hz} className="eq-band">
                <input type="range" min={-20} max={20} step={0.5} value={b.gain_db} aria-label={`Bande ${b.freq_hz} Hz`}
                  onChange={(e) => {
                    const gain = Number(e.target.value);
                    const bands = p.eq.bands.map((x, j) => (j === i ? { ...x, gain_db: gain } : x));
                    setPlayback("eq", { ...p.eq, bands });
                  }} />
                <span className="eq-gain">{b.gain_db > 0 ? `+${b.gain_db}` : `${b.gain_db}`} dB</span>
                <span className="eq-freq">{b.freq_hz >= 1000 ? `${b.freq_hz / 1000}k` : b.freq_hz}</span>
              </div>
            ))}
            <button className="btn ghost sm" onClick={() => setPlayback("eq", { ...p.eq, bands: p.eq.bands.map((b) => ({ ...b, gain_db: 0 })) })}>Tout remettre à plat</button>
          </div>
        ),
      ].filter(Boolean) as React.ReactNode[],
    },
    {
      icon: "palette",
      title: t("Apparence"),
      rows: [
        show(t("Thème"), t("Thème de l'interface (packs embarqués ou déposés dans <data-dir>/themes)")) && (
          <SetRow key="theme" label={t("Thème")} desc={t("Thème de l'interface (packs embarqués ou déposés dans <data-dir>/themes)")}>
            <select value={ap.theme} onChange={(e) => setAppearance("theme", e.target.value)}>
              {(packs?.themes ?? []).map((th) => (
                <option key={th.id} value={th.id}>{t(th.name)}</option>
              ))}
              {!packs?.themes.some((th) => th.id === ap.theme) && (
                <option value={ap.theme}>{ap.theme}</option>
              )}
            </select>
          </SetRow>
        ),
        show(t("Langue"), t("Langue de l'interface (packs embarqués ou déposés dans <data-dir>/locales)")) && (
          <SetRow key="language" label={t("Langue")} desc={t("Langue de l'interface (packs embarqués ou déposés dans <data-dir>/locales)")}>
            <select value={ap.language} onChange={(e) => setAppearance("language", e.target.value)}>
              {(packs?.locales ?? []).map((lo) => (
                <option key={lo.id} value={lo.id}>{t(lo.name)}</option>
              ))}
              {!packs?.locales.some((lo) => lo.id === ap.language) && (
                <option value={ap.language}>{ap.language}</option>
              )}
            </select>
          </SetRow>
        ),
        show(t("Couleur d'accent"), "Couleur principale de l'interface — presets ou #RRGGBB") && (
          <SetRow key="accent" label={t("Couleur d'accent")} desc={t("Couleur principale de l'interface — presets ou #RRGGBB")}>
            <div className="range-ctrl">
              <div className="swatches">
                {ACCENT_PRESETS.map((c) => (
                  <button key={c} type="button" className={`swatch ${ap.accent.toLowerCase() === c ? "active" : ""}`}
                    style={{ background: c }} title={c} aria-label={`${t("Couleur d'accent")} ${c}`}
                    onClick={() => setAppearance("accent", c)} />
                ))}
              </div>
              <input type="color" value={ap.accent} aria-label="Couleur d'accent"
                onChange={(e) => setAppearance("accent", e.target.value)} style={{ width: 44, height: 32, padding: 2, border: "1px solid var(--border)", borderRadius: 6, background: "none" }} />
              <input type="text" value={ap.accent} style={{ width: 90 }}
                onChange={(e) => setAppearance("accent", e.target.value)} onBlur={() => setAppearance("accent", ap.accent)} />
            </div>
          </SetRow>
        ),
        show("Image de fond", "URL d'une image affichée derrière l'interface (optionnel)") && (
          <SetRow key="bg" label="Image de fond" desc="URL d'une image en arrière-plan (vide = aucun)">
            <input type="text" value={ap.background ?? ""} placeholder="https://…/fond.jpg"
              onChange={(e) => setAppearance("background", e.target.value.trim() || null)} />
          </SetRow>
        ),
      ].filter(Boolean) as React.ReactNode[],
    },
    {
      icon: "puzzle",
      title: t("Packs communautaires"),
      rows: ([
        <SetRow key="pack-edit" label={t("Nouveau pack")} desc={t("Thème CSS ou pack de langue JSON — utilisable ensuite dans la section Apparence ci-dessus")}>
          <div className="seg" style={{ marginRight: 8 }}>
            <button className={`seg-btn ${packKind === "theme" ? "active" : ""}`} onClick={() => setPackKind("theme")}>{t("Thème CSS")}</button>
            <button className={`seg-btn ${packKind === "locale" ? "active" : ""}`} onClick={() => setPackKind("locale")}>{t("Langue JSON")}</button>
          </div>
          <input type="text" value={packId} placeholder={packKind === "theme" ? "mon-theme" : "pt-BR"} style={{ width: 120 }}
            onChange={(e) => setPackId(e.target.value)} />
        </SetRow>,
        <div key="pack-body" style={{ margin: "-6px 4px 10px" }}>
          <textarea value={packBody} rows={6} style={{ width: "100%", fontFamily: "monospace", fontSize: 12 }}
            placeholder={packKind === "theme"
              ? t("/* variables CSS : --accent, --bg, --panel, --text… */")
              : t('{ "Clé française": "traduction" }')}
            onChange={(e) => setPackBody(e.target.value)} />
          <button className="btn sm" disabled={packBusy || !packId.trim() || !packBody.trim()} onClick={savePack}>
            <Icon name="download" size={14} /> {packBusy ? t("Enregistrement…") : t("Enregistrer le pack")}
          </button>
        </div>,
        ...(packs?.themes.filter((th) => !th.builtin).map((th) => (
          <SetRow key={`th-${th.id}`} label={`${t("Thème")} : ${th.id}`} desc={t("Pack déposé dans <data-dir>/themes")}>
            <button className="btn ghost sm" onClick={() => delPack("theme", th.id)}><Icon name="trash" size={14} /> {t("Supprimer")}</button>
          </SetRow>
        )) ?? []),
        ...(packs?.locales.filter((lo) => !lo.builtin).map((lo) => (
          <SetRow key={`lo-${lo.id}`} label={`${t("Langue")} : ${lo.id}`} desc={t("Pack déposé dans <data-dir>/locales")}>
            <button className="btn ghost sm" onClick={() => delPack("locale", lo.id)}><Icon name="trash" size={14} /> {t("Supprimer")}</button>
          </SetRow>
        )) ?? []),
      ] as React.ReactNode[]),
    },
    {
      icon: "gear",
      title: t("Système"),
      rows: [
        show("Nom de l'appareil", "Nom affiché sur le réseau et dans l'interface") && (
          <SetRow key="devname" label="Nom de l'appareil" desc="Nom affiché sur le réseau (découverte LAN)">
            <input type="text" value={sy.device_name} style={{ width: 220 }}
              onChange={(e) => setSystem("device_name", e.target.value)} />
          </SetRow>
        ),
        show("Fuseau horaire", "Fuseau horaire IANA, ex. Europe/Paris") && (
          <SetRow key="tz" label="Fuseau horaire" desc="Format IANA, ex. Europe/Paris (cosmétique)">
            <input type="text" value={sy.timezone ?? ""} placeholder="Europe/Paris" style={{ width: 220 }}
              onChange={(e) => setSystem("timezone", e.target.value.trim() || null)} />
          </SetRow>
        ),
        show("Stockage", "Espace utilisé par les données Auris") && (
          <SetRow key="storage" label="Stockage" desc={storage ? `${fmtBytes(storage.used_bytes)} utilisés sur ${fmtBytes(storage.total_bytes)} (libre ${fmtBytes(storage.free_bytes)}) — ${storage.data_dir}` : "chargement…"}>
            <button className="btn ghost sm" onClick={() => api.systemStorage().then(setStorage).catch((e) => say(String(e)))}><Icon name="refresh" size={16} /> Actualiser</button>
          </SetRow>
        ),
        show("Journaux", "Dernières lignes du journal Auris") && (
          <SetRow key="logs" label="Journaux" desc="400 dernières lignes du fichier de logs">
            <button className="btn ghost sm" onClick={async () => { try { setLogs(await api.systemLogs()); } catch (e) { say(e instanceof Error ? e.message : String(e)); } }}><Icon name="list" size={16} /> Voir les logs</button>
          </SetRow>
        ),
        show("Reset usine", "Supprime bibliothèque, playlists, sources et réglages") && (
          <SetRow key="reset" label="Reset usine" desc="Bibliothèque, playlists, sources et réglages remis à zéro (redémarrage recommandé)">
            <button className="btn danger sm" disabled={busy === "reset"} onClick={reset}>{busy === "reset" ? "…" : "Reset usine"}</button>
          </SetRow>
        ),
        show("Redémarrer", "Redémarre la machine hôte (nécessite power_control = true)") && (
          <SetRow key="reboot" label="Redémarrer" desc="Nécessite `power_control = true` dans config.toml">
            <button className="btn ghost sm" disabled={busy === "Redémarrer"} onClick={() => power(api.reboot, "Redémarrer")}>Redémarrer</button>
          </SetRow>
        ),
        show("Éteindre", "Éteint la machine hôte (nécessite power_control = true)") && (
          <SetRow key="shutdown" label="Éteindre" desc="Nécessite `power_control = true` dans config.toml">
            <button className="btn ghost sm" disabled={busy === "Éteindre"} onClick={() => power(api.shutdown, "Éteindre")}>Éteindre</button>
          </SetRow>
        ),
      ].filter(Boolean) as React.ReactNode[],
    },
    {
      icon: "download",
      title: t("Sauvegarde"),
      rows: [
        show("Télécharger la sauvegarde", "ZIP : réglages, favoris, épingles, playlists, historique et partages") && (
          <SetRow key="dl" label="Télécharger la sauvegarde (ZIP)" desc="Réglages + données utilisateur. La bibliothèque n'est pas incluse : elle est reconstruite par un scan.">
            <button className="btn ghost sm" disabled={busy === "backup"} onClick={downloadBackup}>
              <Icon name="download" size={16} /> {busy === "backup" ? "…" : "Télécharger (.zip)"}
            </button>
          </SetRow>
        ),
        show("Restaurer une sauvegarde", "Depuis un fichier ZIP précédemment téléchargé") && (
          <SetRow key="restore" label="Restaurer une sauvegarde" desc="Remplace réglages, favoris, épingles, playlists, historique et partages — l'interface se recharge ensuite">
            <input ref={fileRef} type="file" accept=".zip,application/zip" style={{ display: "none" }}
              onChange={(e) => { const f = e.target.files?.[0]; e.target.value = ""; if (f) void restoreFile(f); }} />
            <button className="btn ghost sm" disabled={busy === "restore"} onClick={() => fileRef.current?.click()}>
              <Icon name="eject" size={16} /> {busy === "restore" ? "…" : "Choisir un .zip…"}
            </button>
          </SetRow>
        ),
        show("WebDAV", "Envoyer ou restaurer la sauvegarde sur un serveur WebDAV (Nextcloud, NAS…)") && (
          <SetRow key="webdav" label="Sauvegarde WebDAV" desc="Envoi : URL du dossier. Restauration : URL complète du fichier .zip. Identifiants facultatifs.">
            <div style={{ display: "flex", flexDirection: "column", gap: 6, alignItems: "flex-end" }}>
              <input type="text" value={davBase} placeholder="https://nas/dav/auris/ (envoi)" style={{ width: 330 }}
                onChange={(e) => setDavBase(e.target.value)} />
              <input type="text" value={davFile} placeholder="https://nas/dav/auris/backup.zip (restauration)" style={{ width: 330 }}
                onChange={(e) => setDavFile(e.target.value)} />
              <div style={{ display: "flex", gap: 6 }}>
                <input type="text" value={davUser} placeholder="utilisateur" style={{ width: 120 }} onChange={(e) => setDavUser(e.target.value)} />
                <input type="password" value={davPass} placeholder="mot de passe" style={{ width: 140 }} onChange={(e) => setDavPass(e.target.value)} />
              </div>
              <div style={{ display: "flex", gap: 6 }}>
                <button className="btn ghost sm" disabled={busy === "dav-push" || !davBase.trim()} onClick={pushWebdav}>
                  <Icon name="server" size={16} /> {busy === "dav-push" ? "…" : "Envoyer"}
                </button>
                <button className="btn ghost sm" disabled={busy === "dav-restore" || !davFile.trim()} onClick={restoreWebdav}>
                  {busy === "dav-restore" ? "…" : "Restaurer"}
                </button>
              </div>
            </div>
          </SetRow>
        ),
      ].filter(Boolean) as React.ReactNode[],
    },
    {
      icon: "star",
      title: t("Last.fm / MusicBrainz"),
      rows: [
        show("Last.fm", "Scrobble automatique et « en cours de lecture » vers Last.fm") && (
          <SetRow key="lf-on" label="Last.fm" desc="Scrobble quand la piste dépasse 50 % de sa durée (ou 4 min, min 30 s) + « en cours de lecture »">
            <Switch checked={lf.enabled} label="Last.fm" onChange={(v) => setLastfm("enabled", v)} />
          </SetRow>
        ),
        show("Clés API Last.fm", "api_key + shared_secret du compte API Last.fm") && (
          <SetRow key="lf-key" label="Clés API Last.fm" desc="Créer un compte API sur last.fm/api/account/create (32 caractères hex)">
            <div style={{ display: "flex", flexDirection: "column", gap: 6, alignItems: "flex-end" }}>
              <input type="text" value={lf.api_key} placeholder="api_key (32 hex)" style={{ width: 310 }}
                onChange={(e) => setLastfm("api_key", e.target.value.trim())} />
              <input type="password" value={lf.shared_secret} placeholder="shared_secret" style={{ width: 310 }}
                onChange={(e) => setLastfm("shared_secret", e.target.value)} />
            </div>
          </SetRow>
        ),
        show("Session Last.fm", "Autorisation du compte (flux token → navigateur → session)") && (
          <SetRow key="lf-auth" label="Session Last.fm" desc={lf.session_key ? "Session enregistrée" : "Aucune session enregistrée"}>
            <div style={{ display: "flex", gap: 6, flexWrap: "wrap", justifyContent: "flex-end" }}>
              <button className="btn ghost sm" disabled={!lf.api_key || !lf.shared_secret} onClick={async () => {
                try {
                  const r = await api.lastfmToken();
                  lfToken.current = r.token;
                  window.open(r.auth_url, "_blank", "noopener");
                  say("Autorisez Auris sur Last.fm dans l'onglet ouvert, puis cliquez sur « Finaliser »");
                } catch (e) { say(e instanceof Error ? e.message : String(e)); }
              }}>1. Autoriser…</button>
              <button className="btn ghost sm" disabled={!lfToken.current} onClick={async () => {
                try {
                  const r = await api.lastfmSession(lfToken.current!);
                  lfToken.current = null;
                  await setLastfm("session_key", r.session_key);
                  say(`Session enregistrée pour ${r.name}`);
                } catch (e) { say(e instanceof Error ? e.message : String(e)); }
              }}>2. Finaliser la session</button>
              <button className="btn ghost sm" disabled={!lf.session_key} onClick={async () => {
                try {
                  const r = await api.lastfmTest();
                  say(`Last.fm connecté : ${r.name ?? "?"} (${r.playcount ?? 0} écoutes)`);
                } catch (e) { say(e instanceof Error ? e.message : String(e)); }
              }}>Tester</button>
            </div>
          </SetRow>
        ),
        show("MusicBrainz", "Fiche recording (sortie, label, tags, ISRC, note) dans l'onglet Infos") && (
          <SetRow key="mb" label="MusicBrainz" desc="API publique WS2 (1 req/s, cache 10 min) — enrichit l'onglet Infos de la lecture en cours">
            <Switch checked={mb.enabled} label="MusicBrainz" onChange={(v) => setMb("enabled", v)} />
          </SetRow>
        ),
      ].filter(Boolean) as React.ReactNode[],
    },
    {
      icon: "wifi",
      title: t("Réseau"),
      rows: [
        show("Nom d'hôte", "Nom d'hôte souhaité pour la machine (Linux uniquement)") && (
          <SetRow key="hostname" label="Nom d'hôte" desc="Appliqué au mieux selon l'OS (Linux uniquement)">
            <input type="text" value={nw.hostname ?? ""} placeholder="auris.local" style={{ width: 220 }}
              onChange={(e) => setNetwork("hostname", e.target.value.trim() || null)} />
          </SetRow>
        ),
        show("Enceinte DLNA", "MediaRenderer UPnP AVTransport BubbleUPnP Hi-Fi Cast diffuser") && (
          <SetRow key="dlna" label="Enceinte DLNA" desc="Auris apparaît comme enceinte DLNA (AVTransport + volume) dans BubbleUPnP, Hi-Fi Cast… pour recevoir de la musique poussée depuis un autre appareil">
            <Switch checked={nw.dlna_renderer} label="DLNA" onChange={(v) => setNetwork("dlna_renderer", v)} />
          </SetRow>
        ),
        show("Interfaces réseau", "État Ethernet/Wi-Fi, route par défaut, activer/désactiver") && (
          <SetRow key="netif" label="Interfaces réseau" desc="État des interfaces (Ethernet + Wi-Fi peuvent être actives en même temps) — activer/désactiver exige les droits root/administrateur">
            <NetInterfaces say={say} />
          </SetRow>
        ),
      ].filter(Boolean) as React.ReactNode[],
    },
    {
      icon: "folder",
      title: t("Sources"),
      rows: [
        show("Découverte de métadonnées musicales", "Tags et pochettes extraits à l'indexation") && (
          <SetRow key="metadata" label="Découverte de métadonnées musicales" desc="Extraction des tags et pochettes lors du scan">
            <Switch checked={so.metadata_discovery} label="métadonnées" onChange={(v) => setSources("metadata_discovery", v)} />
          </SetRow>
        ),
      ].filter(Boolean) as React.ReactNode[],
    },
    {
      icon: "share",
      title: t("Partage"),
      rows: [
        show("Liens de partage", "URL publiques /share/<token> pour écouter une piste, un album ou une playlist depuis un autre appareil") && (
          <SetRow key="shares" label="Liens de partage" desc={shares.length ? `${shares.length} lien(s) actif(s)` : "aucun lien actif"}>
            <button className="btn ghost sm" onClick={loadShares}><Icon name="refresh" size={16} /> Actualiser</button>
          </SetRow>
        ),
        show("API Subsonic", "Serveur compatible Subsonic (lecture seule) pour clients tiers : DSub, Ultrasonic, Strawberry…") && (
          <SetRow key="subsonic" label="API Subsonic" desc="Serveur compatible Subsonic (lecture seule) pour clients tiers : DSub, Ultrasonic, Strawberry…">
            <code>{window.location.origin}/rest</code>
          </SetRow>
        ),
        show("Serveur MPD", "Protocole MPD sur port 6600 pour clients tiers : Cantata, Mpdroid, MALP, mconnectHD…") && (
          <SetRow key="mpd" label="Serveur MPD" desc="Protocole MPD (Music Player Daemon) pour clients tiers : Cantata, Mpdroid, MALP, mconnectHD…">
            <code>{window.location.hostname}:6600</code>
          </SetRow>
        ),
      ].filter(Boolean) as React.ReactNode[],
    },
  ].map((sec) => ({ ...sec, rows: sec.rows.filter(Boolean) }));

  const visible = sections.filter((s) => s.rows.length > 0);

  return (
    <div className="settings">
      {visible.length === 0 && <div className="empty">{t("Aucun réglage ne correspond à « {q} ».", { q })}</div>}
      {visible.map((sec) => (
        <SetGroup key={sec.title} icon={sec.icon} title={sec.title}
          open={qn ? true : openSec === sec.title}
          onToggle={() => setOpenSec(openSec === sec.title ? null : sec.title)}>
          {sec.rows}
        </SetGroup>
      ))}
      {show("DoP", "Régler le DAC pour la lecture DSD (fichiers .dsf) en DoP") && dop?.windows && (
        <SetGroup icon="chip" title="DoP — DAC USB (Windows)" open={qn !== "" || openSec === "dop"} onToggle={() => setOpenSec(openSec === "dop" ? null : "dop")}>
        <section className="set-card">
          <h2>DoP — lecture DSD sur DAC USB (Windows)</h2>
          {(() => {
            const devName = dop.current_device;
            const dev = dop.devices.find((d) => devName != null && sameDevice(d.name, devName)) ?? dop.devices[0] ?? null;
            const mix = dev?.mix ?? null;
            const def = dev?.default_format ?? null;
            // « Prêt » = mode exclusif actif (mixeur contourné), ou Default Format
            // (adopté au prochain rebranchement) en PCM entier à la porteuse.
            const ok = dop.exclusive_enabled || (def != null && !def.is_float && def.rate === dopRate);
            return (
              <>
                <SetRow label="État WASAPI du DAC"
                  desc={dev ? `${dev.name} — mix actuel : ${fmtMix(mix)} ; format adopté au prochain rebranchement : ${fmtMix(def)}` : "Aucun périphérique audio détecté"}>
                  {mix && <span className={`pill ${ok ? "ok" : "warn"}`}>{ok ? `Prêt DoP ${dopRate / 1000} kHz` : "DoP non actif"}</span>}
                </SetRow>
                <SetRow label="Mode exclusif WASAPI (bit-perfect)"
                  desc={dop.exclusive_supported
                    ? "Le DAC est ouvert en mode EXCLUSIF : aucun moteur audio Windows entre auris et le pilote, le flux arrive intact (PCM natif au format de la source, ou trames DoP). Si le DAC refuse, retour automatique au mode partagé."
                    : "Ce périphérique n'accepte aucun format en mode exclusif — le mode partagé (cpal) est utilisé."}>
                  <Switch checked={dop.exclusive_enabled} label="Mode exclusif WASAPI"
                    onChange={async (v) => {
                      try { await api.setExclusive(v); loadDop(); say(v ? "Mode exclusif WASAPI activé (bit-perfect)" : "Mode exclusif désactivé — mode partagé"); }
                      catch (e) { say(e instanceof Error ? e.message : String(e)); }
                    }} />
                </SetRow>
                <SetRow label="Porteuse DoP à appliquer"
                  desc="Écrit le Default Format du DAC en PCM 24 bits à cette fréquence (à faire au repos, sans lecture en cours). Devient effectif au rebranchement du DAC ou au redémarrage du service Audio Windows.">
                  <div style={{ display: "flex", gap: 8, alignItems: "center" }}>
                    <select value={dopRate} onChange={(e) => setDopRate(Number(e.target.value))}>
                      <option value={176400}>DSD64 → 24 bits / 176400 Hz</option>
                      <option value={352800}>DSD128 → 24 bits / 352800 Hz</option>
                    </select>
                    <button className="btn sm" disabled={dopBusy} onClick={async () => {
                      setDopBusy(true);
                      try { say((await api.dopSetup(dopRate, dop.current_device)).message); loadDop(); }
                      catch (e) { say(e instanceof Error ? e.message : String(e)); }
                      finally { setDopBusy(false); }
                    }}>{dopBusy ? "…" : `Appliquer 24 bits / ${dopRate} Hz`}</button>
                  </div>
                </SetRow>
                <SetRow label="Appliquer maintenant"
                  desc="Redémarre le service Audio Windows (invite UAC) pour adopter le format sans rebrancher le DAC. Alternative : rebrancher le câble USB du DAC.">
                  <button className="btn sm" disabled={dopBusy} onClick={async () => {
                    setDopBusy(true);
                    try { say((await api.restartAudio()).message); loadDop(); }
                    catch (e) { say(e instanceof Error ? e.message : String(e)); }
                    finally { setDopBusy(false); }
                  }}>Redémarrer Audio Windows</button>
                </SetRow>
              </>
            );
          })()}
        </section>
        </SetGroup>
      )}
      {show("Liens de partage") && (
        <SetGroup icon="link" title={t("Liens de partage actifs")} open={qn !== "" || openSec === "shares"} onToggle={() => setOpenSec(openSec === "shares" ? null : "shares")}>
        <section className="set-card">
          <h2>Liens de partage actifs</h2>
          {shares.length === 0 && <div className="empty">Aucun lien — partagez une piste (barre de lecture), un album ou une playlist.</div>}
          {shares.map((sh) => (
            <div key={sh.token} className="share-row">
              <span className="badge">{sh.kind}</span>
              <span className="share-title">{sh.title}</span>
              <code className="share-url">{api.shareUrl(sh.token)}</code>
              <button className="icon-btn" title="Copier le lien" onClick={async () => { await navigator.clipboard?.writeText(api.shareUrl(sh.token)).catch(() => {}); say("Lien copié dans le presse-papiers"); }}><Icon name="copy" size={16} /></button>
              <button className="icon-btn" title="Ouvrir dans un onglet" onClick={() => window.open(api.shareUrl(sh.token), "_blank")}><Icon name="link" size={16} /></button>
              <button className="icon-btn danger" title="Révoquer ce lien" onClick={async () => { try { await api.deleteShare(sh.token); say("Lien révoqué"); loadShares(); } catch (e) { say(e instanceof Error ? e.message : String(e)); } }}><Icon name="trash" size={16} /></button>
            </div>
          ))}
        </section>
        </SetGroup>
      )}
      {logs !== null && (
        <div className="modal-bg" onClick={() => setLogs(null)}>
          <div className="modal wide" onClick={(e) => e.stopPropagation()}>
            <div className="modal-head">
              <h2>Journaux Auris</h2>
              <button className="icon-btn" onClick={() => setLogs(null)}><Icon name="close" size={20} /></button>
            </div>
            <pre className="logs">{logs}</pre>
          </div>
        </div>
      )}
      {show("Rip CD", "Ripper un CD audio avec whipper (AccurateRip)") && (
        <SetGroup icon="disc" title={`${t("Rip CD")} — whipper`} open={qn !== "" || openSec === "rip"} onToggle={() => setOpenSec(openSec === "rip" ? null : "rip")}>
          <RipCdCard say={say} />
        </SetGroup>
      )}
      {show("Spotify Connect", "Écouter Spotify sur Auris depuis l'app Spotify (librespot)") && (
        <SetGroup icon="spotify" title={`${t("Spotify Connect")} — librespot`} open={qn !== "" || openSec === "spotify"} onToggle={() => setOpenSec(openSec === "spotify" ? null : "spotify")}>
          <SpotifyCard settings={settings} updateSettings={updateSettings} say={say} />
        </SetGroup>
      )}
      {show("CamillaDSP", "Déléguer le traitement audio (convolution, correction de pièce) à une instance CamillaDSP") && (
        <SetGroup icon="wave" title="CamillaDSP — convolution / correction de pièce" open={qn !== "" || openSec === "camilla"} onToggle={() => setOpenSec(openSec === "camilla" ? null : "camilla")}>
          <CamillaCard settings={settings} updateSettings={updateSettings} say={say} />
        </SetGroup>
      )}
    </div>
  );
}

// ---------- Spotify Connect (librespot) — #24 ----------

function SpotifyCard({ settings, updateSettings, say }: {
  settings: Settings;
  updateSettings: (patch: (s: Settings) => Settings, ok?: string) => Promise<void>;
  say: (m: string) => void;
}) {
  const sp = settings.spotify;
  const [st, setSt] = useState<SpotStatus | null>(null);
  const [busy, setBusy] = useState("");
  const load = useCallback(() => { api.spotifyStatus().then(setSt).catch(() => {}); }, []);
  useEffect(() => {
    load();
    const iv = setInterval(() => {
      setSt((cur) => {
        if (cur?.running) load();
        return cur;
      });
    }, 3000);
    return () => clearInterval(iv);
  }, [load]);
  const set = <K extends keyof SpotifySettings>(k: K, v: SpotifySettings[K]) =>
    updateSettings((s) => ({ ...s, spotify: { ...s.spotify, [k]: v } }));

  const run = async (label: string, fn: () => Promise<SpotStatus>) => {
    setBusy(label);
    try { setSt(await fn()); say(`${label} OK`); }
    catch (e) { say(e instanceof Error ? e.message : String(e)); }
    finally { setBusy(""); load(); }
  };

  return (
    <section className="set-card">
      <h2>{t("Spotify Connect")} — librespot</h2>
      <SetRow label="Spotify Connect" desc="Auris apparaît comme enceinte dans l'app Spotify (compte Spotify requis). À la lecture, Auris met sa propre file en pause.">
        <Switch checked={sp.enabled} label="Spotify Connect" onChange={(v) => set("enabled", v)} />
      </SetRow>
      <SetRow label="Compte Spotify" desc="Identifiants (auth legacy) ou chemin vers un credentials.json librespot (recommandé, OAuth)">
        <div style={{ display: "flex", flexDirection: "column", gap: 6, alignItems: "flex-end" }}>
          <input type="text" value={sp.username} placeholder="nom d'utilisateur" style={{ width: 280 }}
            onChange={(e) => set("username", e.target.value)} />
          <input type="password" value={sp.password} placeholder="mot de passe (ou vide si credentials.json)" style={{ width: 280 }}
            onChange={(e) => set("password", e.target.value)} />
          <input type="text" value={sp.credentials_path} placeholder="chemin credentials.json (optionnel)" style={{ width: 280 }}
            onChange={(e) => set("credentials_path", e.target.value)} />
        </div>
      </SetRow>
      <SetRow label="Nom de l'enceinte" desc={`Affiché dans l'app Spotify (défaut : ${settings.system.device_name})`}>
        <input type="text" value={sp.device_name} placeholder={settings.system.device_name} style={{ width: 220 }}
          onChange={(e) => set("device_name", e.target.value)} />
      </SetRow>
      <SetRow label="Débit" desc="Qualité du flux Spotify">
        <select value={sp.bitrate} onChange={(e) => set("bitrate", Number(e.target.value))} style={{ width: 120 }}>
          <option value={96}>96 kbps</option>
          <option value={160}>160 kbps</option>
          <option value={320}>320 kbps</option>
        </select>
      </SetRow>
      <SetRow label="Sortie audio de librespot" desc="Backend librespot (vide = auto) et périphérique ALSA (backend alsa)">
        <div style={{ display: "flex", gap: 6 }}>
          <input type="text" value={sp.backend} placeholder="auto (alsa/rodio)" style={{ width: 150 }}
            onChange={(e) => set("backend", e.target.value)} />
          <input type="text" value={sp.alsa_device} placeholder="default" style={{ width: 110 }}
            onChange={(e) => set("alsa_device", e.target.value)} />
        </div>
      </SetRow>
      <SetRow label="État" desc={!st
          ? t("Chargement…")
          : !st.available
            ? "librespot introuvable dans le PATH — installez-le (ex. `apt install librespot` ou binaire GitHub) ou définissez AURIS_LIBRESPOT_BIN"
            : `${st.version ? `librespot ${st.version} · ` : "librespot · "}${st.backend || "auto"} · ${st.bitrate} kbps${st.last_event ? ` · dernier événement : ${st.last_event}` : ""}${st.track_id ? ` · ${st.track_id}` : ""}${st.error ? ` · ${st.error}` : ""}`}>
        <div style={{ display: "flex", gap: 8, alignItems: "center" }}>
          {st && (
            <span className={`pill ${st.error ? "warn" : st.running ? "ok" : ""}`}>
              {st.running ? `En ligne (pid ${st.pid ?? "?"})` : st.available ? "Arrêté" : "Indisponible"}
            </span>
          )}
          <button className="btn sm" disabled={!sp.enabled || !st?.available || st?.running || busy !== ""}
            onClick={() => run("Démarrage", () => api.spotifyStart())}>
            <Icon name="play" size={16} /> {t("Démarrer")}
          </button>
          <button className="btn danger sm" disabled={!st?.running || busy !== ""} onClick={() => run("Arrêt", () => api.spotifyStop())}>
            {t("Arrêter")}
          </button>
        </div>
      </SetRow>
      {st && st.log.length > 0 && (
        <SetRow label="Journal librespot" desc="200 dernières lignes">
          <pre className="logs" style={{ maxWidth: 560, maxHeight: 180, margin: 0 }}>{st.log.slice(-30).join("\n")}</pre>
        </SetRow>
      )}
    </section>
  );
}

// ---------- Pont CamillaDSP (convolution / correction de pièce) — #82 ----------

function CamillaCard({ settings, updateSettings, say }: {
  settings: Settings;
  updateSettings: (patch: (s: Settings) => Settings, ok?: string) => Promise<void>;
  say: (m: string) => void;
}) {
  const ca = settings.camilla;
  const [st, setSt] = useState<CamillaStatusResponse | null>(null);
  const [busy, setBusy] = useState(false);
  const load = useCallback(() => { api.camillaStatus().then(setSt).catch(() => {}); }, []);
  useEffect(() => { load(); }, [load]);
  const set = <K extends keyof CamillaSettings>(k: K, v: CamillaSettings[K]) =>
    updateSettings((s) => ({ ...s, camilla: { ...s.camilla, [k]: v } }));

  const live = st?.live ?? null;
  const profiles = live?.profiles ?? [];
  const currentProfile = ca.profile ?? live?.active_profile ?? "";
  const statusPill = !ca.enabled
    ? <span className="pill">Désactivé</span>
    : !live
      ? <span className="pill">…</span>
      : live.reachable
        ? <span className="pill ok">Joignable{live.version ? ` · v${live.version}` : ""}{live.state ? ` · ${live.state}` : ""}</span>
        : <span className="pill warn">Injoignable{live.error ? ` — ${live.error}` : ""}</span>;

  return (
    <section className="set-card">
      <h2>CamillaDSP — convolution / correction de pièce</h2>
      <SetRow label="Déléguer le DSP à CamillaDSP"
        desc="Quand une instance CamillaDSP est joignable, Auris lui route le flux (le DSP interne EQ/loudness est alors court-circuité) : filtres FIR, convolution, correction de pièce. Sinon, repli sur le DSP interne.">
        <Switch checked={ca.enabled} label="Pont CamillaDSP" onChange={(v) => set("enabled", v)} />
      </SetRow>
      <SetRow label="Mode de pont" desc="Loopback : le flux va sur le périphérique de capture de CamillaDSP (ex. boucle ALSA « camilladsp »). Pipe : PCM brut f32le écrit dans un FIFO. Websocket : contrôle du profil uniquement, la chaîne audio reste interne.">
        <select value={ca.mode} onChange={(e) => set("mode", e.target.value as CamillaSettings["mode"])}>
          <option value="loopback">Loopback (périphérique de capture)</option>
          <option value="pipe">Pipe nommé (FIFO)</option>
          <option value="websocket">Websocket seul (contrôle)</option>
        </select>
      </SetRow>
      <SetRow label="Websocket CamillaDSP" desc="API de contrôle de CamillaDSP (port 1234 par défaut)">
        <input type="text" value={ca.ws_url} placeholder="ws://127.0.0.1:1234" style={{ width: 260 }}
          onChange={(e) => set("ws_url", e.target.value)} />
      </SetRow>
      {ca.mode !== "websocket" && (
        <SetRow label={ca.mode === "pipe" ? "FIFO de capture" : "Périphérique de capture"}
          desc={ca.mode === "pipe"
            ? "Chemin du FIFO lu par CamillaDSP (capture File/RawFile, format FLOAT32LE à la cadence du flux)"
            : "Nom du périphérique audio que CamillaDSP écoute en capture (ex. « camilladsp » avec une boucle ALSA), affiché aussi dans la liste des périphériques audio"}>
          <input type="text" value={ca.capture_device} placeholder={ca.mode === "pipe" ? "/tmp/camilla.fifo" : "camilladsp"} style={{ width: 260 }}
            onChange={(e) => set("capture_device", e.target.value)} />
        </SetRow>
      )}
      <SetRow label="Repli sur le DSP interne" desc="Si CamillaDSP est injoignable ou le périphérique de capture indisponible : lecture sur la sortie normale avec l'EQ interne, plutôt que silence">
        <Switch checked={ca.fallback_internal} label="Repli DSP interne" onChange={(v) => set("fallback_internal", v)} />
      </SetRow>
      <SetRow label="Profil CamillaDSP" desc={live?.active_profile ? `Profil actif côté CamillaDSP : ${live.active_profile}` : "Sélectionnez le profil (fichier de config) à activer — ex. correction de pièce « salon »"}>
        <div style={{ display: "flex", gap: 8, alignItems: "center" }}>
          <select value={currentProfile} disabled={!live?.reachable || busy} style={{ minWidth: 200 }}
            onChange={async (e) => {
              const name = e.target.value;
              setBusy(true);
              try {
                const r = await api.camillaProfile(name);
                setSt(r);
                await updateSettings((s) => ({ ...s, camilla: { ...s.camilla, profile: name || null } }));
                say(name ? `Profil « ${name} » activé sur CamillaDSP` : "Profil CamillaDSP désélectionné");
              } catch (err) { say(err instanceof Error ? err.message : String(err)); load(); }
              finally { setBusy(false); }
            }}>
            {currentProfile === "" && <option value="">(profil courant de CamillaDSP)</option>}
            {profiles.map((p) => <option key={p} value={p}>{p}</option>)}
            {currentProfile !== "" && !profiles.includes(currentProfile) && <option value={currentProfile}>{currentProfile}</option>}
          </select>
        </div>
      </SetRow>
      <SetRow label="État" desc={st
          ? `Sortie courante : ${st.device ?? "—"}${st.delegated ? " · DSP délégué à CamillaDSP" : " · DSP interne"}`
          : t("Chargement…")}>
        <div style={{ display: "flex", gap: 8, alignItems: "center" }}>
          {statusPill}
          {st?.delegated && <span className="pill ok">Flux délégué{st.engine.active_profile ? ` · ${st.engine.active_profile}` : ""}</span>}
          <button className="btn sm" disabled={busy} onClick={async () => {
            setBusy(true);
            try { setSt(await api.camillaRefresh()); say("CamillaDSP re-sondé"); }
            catch (e) { say(e instanceof Error ? e.message : String(e)); }
            finally { setBusy(false); }
          }}>{t("Re-sonder")}</button>
        </div>
      </SetRow>
    </section>
  );
}

// ---------- Rip CD (whipper) — #27 ----------

function RipCdCard({ say }: { say: (m: string) => void }) {
  const [st, setSt] = useState<RipStatus | null>(null);
  const [busy, setBusy] = useState("");
  const load = useCallback(() => { api.ripStatus().then(setSt).catch(() => {}); }, []);
  useEffect(() => {
    load();
    const iv = setInterval(() => {
      setSt((cur) => {
        if (cur && (cur.phase === "ripping" || cur.phase === "info")) load();
        return cur;
      });
    }, 2000);
    return () => clearInterval(iv);
  }, [load]);

  const run = async (label: string, fn: () => Promise<RipStatus>) => {
    setBusy(label);
    try { setSt(await fn()); say(`${label} OK`); }
    catch (e) { say(e instanceof Error ? e.message : String(e)); }
    finally { setBusy(""); load(); }
  };

  const ripping = st?.phase === "ripping" || st?.phase === "info";
  const phaseLabel: Record<string, string> = {
    idle: "Prêt", info: "Analyse du CD…", ripping: "Rip en cours…", done: "Terminé", error: "Erreur",
  };
  return (
    <section className="set-card">
      <h2>{t("Rip CD")} — whipper</h2>
      {!st && <div className="empty">{t("Chargement…")}</div>}
      {st && (
        <>
          <SetRow label="État" desc={!st.whipper_available
            ? "whipper introuvable dans le PATH — installez-le (ex. `apt install whipper`) pour activer le rip CD"
            : `${st.whipper_version || "whipper"} détecté${st.device ? ` · lecteur : ${st.device}` : ""}${st.output_dir ? ` · sortie : ${st.output_dir}` : ""}`}>
            <span className={`pill ${st.phase === "error" ? "warn" : st.phase === "done" ? "ok" : ""}`}>{t(phaseLabel[st.phase] ?? st.phase)}</span>
          </SetRow>
          <SetRow label="Actions" desc="Analyser lit la table des matières ; Ripper lance whipper (AccurateRip) dans la première source locale active">
            <div style={{ display: "flex", gap: 8, flexWrap: "wrap" }}>
              <button className="btn ghost sm" disabled={!st.whipper_available || ripping || busy !== ""} onClick={() => run("Analyse", () => api.ripInfo(st.device))}>
                <Icon name="refresh" size={16} /> {t("Analyser le CD")}
              </button>
              <button className="btn sm" disabled={!st.whipper_available || ripping || busy !== ""} onClick={() => run("Rip", () => api.ripStart(st.device))}>
                <Icon name="disc" size={16} /> {t("Ripper le CD")}
              </button>
              <button className="btn danger sm" disabled={!ripping || busy !== ""} onClick={() => run("Arrêt", () => api.ripStop())}>{t("Arrêter")}</button>
              <button className="btn ghost sm" disabled={busy !== ""} onClick={async () => { try { const r = await api.ripEject(st.device); say(r.ok ? "Tiroir éjecté" : `Éjection échouée : ${r.stderr}`); } catch (e) { say(e instanceof Error ? e.message : String(e)); } }}>{t("Éjecter")}</button>
            </div>
          </SetRow>
          {(ripping || st.progress > 0) && (
            <SetRow label="Progression" desc={st.total_tracks > 0 ? `Piste ${st.current_track}/${st.total_tracks}` : ""}>
              <div className="rip-progress" style={{ width: 220 }}>
                <div className="rip-progress-fill" style={{ width: `${Math.min(100, st.progress)}%` }} />
              </div>
              <span className="badge">{Math.round(st.progress)} %</span>
            </SetRow>
          )}
          {st.toc.length > 0 && (
            <SetRow label="Table des matières" desc={`${st.toc.length} piste(s)`}>
              <div style={{ maxHeight: 160, overflowY: "auto", minWidth: 260 }}>
                {st.toc.map((tr) => (
                  <div key={tr.n} className="browse-row" style={{ cursor: "default", padding: "4px 8px" }}>
                    <span className="row-ico"><Icon name="note" size={16} /></span>
                    <span className="name">{tr.n}. {tr.title ?? ""}</span>
                    <span className="meta">{fmtTime(tr.duration_secs)}</span>
                  </div>
                ))}
              </div>
            </SetRow>
          )}
          {st.error && <SetRow label="Erreur" desc={st.error}><span className="pill warn">!</span></SetRow>}
          {st.log.length > 0 && (
            <SetRow label="Journal whipper" desc="Dernières lignes de sortie">
              <pre className="logs" style={{ maxHeight: 140, minWidth: 320 }}>{st.log.slice(-30).join("\n")}</pre>
            </SetRow>
          )}
        </>
      )}
    </section>
  );
}

// ---------- Bannière DoP (lecture DSD sur DAC USB, Windows) ----------

/** Correspondance nom cpal ("Casque (2- iBasso-DC03-Pro)") vs nom WASAPI ("Casque (iBasso-DC03-Pro)"). */
function sameDevice(a: string, b: string): boolean {
  const norm = (s: string) => s.toLowerCase().replace(/[^a-z0-9]/g, "");
  const x = norm(a), y = norm(b);
  return x.length > 0 && y.length > 0 && (x.includes(y) || y.includes(x));
}

function DopBanner({ carrier, dop, deviceName, backend, say, onChanged }: { carrier: number; dop: DopStatus; deviceName: string | null; backend: string | null; say: (m: string) => void; onChanged: () => void }) {
  const [busy, setBusy] = useState(false);
  const dev = dop.devices.find((d) => deviceName != null && sameDevice(d.name, deviceName));
  const mix = dev?.mix ?? null;
  // Le mix format Windows ne concerne que la sortie partagée (backend cpal) :
  // en WASAPI exclusif ou en lecture navigateur, le flux DoP ouvre le device
  // (ou le navigateur) directement — le mixeur est contourné, l'avertissement
  // n'a plus lieu d'être.
  if (backend !== "cpal") return null;
  // RAS si le mix tourne déjà à la fréquence de la porteuse : aucun
  // rééchantillonnage, donc aucun risque de bruit (même en float 32 bits).
  // Avertissement seulement si les fréquences diffèrent (le flux DoP sera
  // rééchantillonné -> bruit) ou si le pilote refuserait le format.
  if (!dop.windows || !mix || mix.rate === carrier) return null;
  const run = async (fn: () => Promise<{ message: string }>) => {
    setBusy(true);
    try { say((await fn()).message); onChanged(); }
    catch (e) { say(e instanceof Error ? e.message : String(e)); }
    finally { setBusy(false); }
  };
  return (
    <div className="dop-banner">
      <span className="pill warn">DoP</span>
      <span className="dop-text">
        Le DAC n'est pas réglé pour le DoP (mix actuel : <b>{fmtMix(mix)}</b>, porteuse : <b>24 bits / {carrier} Hz</b>) — le .dsf risque de sortir en bruit.
        Appliquez le format au repos, puis rebranchez le DAC ou redémarrez Audio Windows.
      </span>
      <button className="btn sm" disabled={busy} onClick={() => run(() => api.dopSetup(carrier, deviceName))}>
        Appliquer 24 bits / {carrier} Hz
      </button>
      <button className="btn ghost sm" disabled={busy} onClick={() => run(() => api.restartAudio())}>
        Redémarrer Audio Windows
      </button>
    </div>
  );
}

// ---------- Barre de lecture ----------

/// Mode affichage plein écran « de loin » pour écran HDMI (#42) :
/// pochette XXL + horloge + contrôles tactiles, curseur masqué après 3 s d'inactivité.
function DisplayView({ state, act, onExit }: { state: PlayerState | null; act: (fn: () => Promise<unknown>, ok?: string) => Promise<void>; onExit: () => void }) {
  const [now, setNow] = useState(new Date());
  const [cursorOn, setCursorOn] = useState(true);
  const hideTimer = useRef<number | null>(null);
  const poke = useCallback(() => {
    setCursorOn(true);
    if (hideTimer.current) window.clearTimeout(hideTimer.current);
    hideTimer.current = window.setTimeout(() => setCursorOn(false), 3000);
  }, []);
  useEffect(() => {
    const id = window.setInterval(() => setNow(new Date()), 1000);
    poke();
    return () => { window.clearInterval(id); if (hideTimer.current) window.clearTimeout(hideTimer.current); };
  }, [poke]);

  const cur = state?.current;
  const playing = state?.status === "playing";
  const live = state?.is_live ?? false;
  const pos = state?.position_secs ?? 0;
  const dur = state?.duration_secs ?? cur?.duration_secs ?? 0;
  const vol = state?.volume ?? 0;

  return (
    <div className={`display${cursorOn ? "" : " nocursor"}`} onMouseMove={poke} onTouchStart={poke}>
      <div className="display-cover">
        {cur?.has_cover ? <img src={api.coverUrl(cur.id)} alt="" /> : <Icon name={live ? "radio" : "note"} size={160} />}
      </div>
      <div className="display-info">
        <div className="display-clock">{now.toLocaleTimeString([], { hour: "2-digit", minute: "2-digit" })}</div>
        <div className="display-date">{now.toLocaleDateString([], { weekday: "long", day: "numeric", month: "long" })}</div>
        <div className="display-title">{cur ? cur.title : t("Aucune lecture")}</div>
        {cur && <div className="display-artist">{live && state?.stream_title ? state.stream_title : cur.artist}</div>}
        {cur?.album && <div className="display-album">{cur.album}</div>}
        {live ? (
          <div className="display-live"><span className="badge live">{t("EN DIRECT")}</span><span className="num-time">{fmtTime(pos)}</span></div>
        ) : (
          dur > 0 && (
            <div className="display-seek">
              <span>{fmtTime(pos)}</span>
              <input type="range" min={0} max={dur} step={0.1} value={Math.min(pos, dur)} aria-label="Progression"
                onChange={(e) => act(() => api.seek(Number(e.target.value)))} />
              <span>{fmtTime(dur)}</span>
            </div>
          )
        )}
        <div className="display-controls">
          <button title={t("Précédent")} onClick={() => act(() => api.prev())}><Icon name="prev" size={36} /></button>
          <button className="big" title={playing ? t("Pause") : t("Lecture")} onClick={() => act(() => (playing ? api.pause() : api.resume()))}>
            {playing ? <Icon name="pause" size={44} /> : <Icon name="play" size={44} />}
          </button>
          <button title={t("Suivant")} onClick={() => act(() => api.next())}><Icon name="next" size={36} /></button>
          <button title={state?.muted ? t("Réactiver le son") : t("Couper le son")} onClick={() => act(() => api.mute(!state?.muted))}>
            <Icon name="speaker" size={30} />
          </button>
          <button title={t("Volume -")} onClick={() => act(() => api.volume(Math.max(0, vol - 5)))}><Icon name="minus" size={28} /></button>
          <span className="display-vol">{vol}%</span>
          <button title={t("Volume +")} onClick={() => act(() => api.volume(Math.min(100, vol + 5)))}><Icon name="plus" size={28} /></button>
          <button title={t("Quitter le mode affichage (Échap)")} onClick={onExit}><Icon name="close" size={30} /></button>
        </div>
      </div>
    </div>
  );
}

/// Modale « Signal path » façon Roon (#77) : la chaîne de traitement appliquée
/// au flux en cours (source → format → bit-perfect → DSD → loudness → EQ →
/// volume → sortie), rafraîchie en direct.
function SignalPathModal({ onClose, onOpenSettings }: { onClose: () => void; onOpenSettings: () => void }) {
  const [sp, setSp] = useState<SignalPath | null>(null);
  useEffect(() => {
    let dead = false;
    const load = async () => { try { const d = await api.signalPath(); if (!dead) setSp(d); } catch { /* ignore */ } };
    load();
    const iv = setInterval(load, 2000);
    return () => { dead = true; clearInterval(iv); };
  }, []);
  const fmt = sp && sp.format_kind
    ? { kind: sp.format_kind, sample_rate: sp.sample_rate ?? undefined, bits: sp.bits ?? undefined, channels: sp.channels ?? undefined }
    : null;
  const stages: { label: string; detail: string; on: boolean }[] = !sp ? [] : [
    { label: t("Source"), detail: sp.live ? `Radio web — ${sp.uri}` : sp.uri, on: true },
    { label: t("Format natif"), detail: fmt ? fmtFormat(fmt) : "—", on: fmt != null },
    { label: t("Bit-perfect"), detail: sp.resampling ? t("Rééchantillonnage autorisé vers la cadence de la sortie") : t("Aucun rééchantillonnage — flux bit-perfect"), on: !sp.resampling },
    ...(sp.format_kind === "dsd" ? [{ label: "DSD", detail: sp.dsd_mode === "dop" ? "DSD over PCM (DoP) vers le DAC" : "DSD converti en PCM haute fréquence", on: true }] : []),
    ...(sp.output_mode === "local" ? [{ label: t("WASAPI exclusif"), detail: sp.exclusive ? t("Actif — le flux prend le DAC en exclusif") : t("Inactif — mode partagé"), on: sp.exclusive }] : []),
    { label: "DSP", detail: sp.dsp.startsWith("CamillaDSP") ? `Délégué à ${sp.dsp} — convolution / correction de pièce externe` : "Interne — EQ paramétrique + loudness R128", on: sp.dsp.startsWith("CamillaDSP") },
    { label: "Loudness R128", detail: sp.loudness && !sp.dsp.startsWith("CamillaDSP") ? `Cible ${sp.loudness_target_lufs} LUFS${sp.loudness_gain_db != null ? ` · gain ${sp.loudness_gain_db.toFixed(1)} dB sur ce morceau` : ""}` : t("Désactivée"), on: sp.loudness && !sp.dsp.startsWith("CamillaDSP") },
    { label: t("Égaliseur"), detail: sp.eq_enabled && sp.eq_bands.length > 0 ? sp.eq_bands.map((b) => `${b.freq_hz} Hz ${b.gain_db > 0 ? "+" : ""}${b.gain_db} dB`).join(", ") : t("Plat / désactivé"), on: sp.eq_enabled && sp.eq_bands.length > 0 },
    { label: t("Enchaînements"), detail: [sp.gapless ? "gapless" : null, sp.crossfade_secs > 0 ? `crossfade ${sp.crossfade_secs} s` : null].filter(Boolean).join(" · ") || t("direct"), on: sp.gapless || sp.crossfade_secs > 0 },
    { label: t("Volume"), detail: sp.volume_mode === "disabled" ? t("Bit-perfect — volume réglé sur l'ampli") : `${t("logiciel")} · ${sp.volume_curve === "logarithmic" ? t("courbe logarithmique") : t("courbe linéaire")} · ${sp.muted ? t("muet") : `${sp.volume}/${sp.max_volume}`}`, on: sp.volume_mode === "software" && !sp.muted },
    { label: t("Sortie"), detail: `${sp.device || t("DAC par défaut")} · ${sp.output_backend}${sp.output_mode === "browser" ? " · navigateur" : ""}`, on: true },
  ];
  return (
    <div className="modal-bg" onClick={onClose}>
      <div className="modal wide" onClick={(e) => e.stopPropagation()}>
        <div className="modal-head">
          <h2><Icon name="sliders" size={20} /> Signal path</h2>
          <button className="icon-btn" title={t("Fermer")} onClick={onClose}><Icon name="close" size={18} /></button>
        </div>
        {sp ? (
          <>
            <div className="sp-track" title={sp.uri}>{sp.live ? t("EN DIRECT") : sp.title}{sp.stream_title ? ` — ${sp.stream_title}` : ""}</div>
            <div className="sp-stages">
              {stages.map((s) => (
                <div key={s.label} className={`sp-stage ${s.on ? "on" : ""}`}>
                  <span className="sp-dot" />
                  <span className="sp-label">{s.label}</span>
                  <span className="sp-detail" title={s.detail}>{s.detail}</span>
                </div>
              ))}
            </div>
            <div className="modal-foot">
              <button className="btn primary" onClick={onOpenSettings}>{t("Réglages de lecture")}</button>
            </div>
          </>
        ) : <div className="empty">{t("Chargement…")}</div>}
      </div>
    </div>
  );
}

function PlayerBar({ state, devices, act, say, alarms, refreshDevices, npOpen, onToggleNp, onToggleDisplay, onOpenSignalPath }: { state: PlayerState | null; devices: AudioDevice[]; act: (fn: () => Promise<unknown>, ok?: string) => Promise<void>; say: (m: string) => void; alarms: AlarmsInfo | null; refreshDevices: () => Promise<void>; npOpen: boolean; onToggleNp: () => void; onToggleDisplay: () => void; onOpenSignalPath: () => void }) {
  const playing = state?.status === "playing";
  const [menuOpen, setMenuOpen] = useState(false);
  const cur = state?.current;
  const pos = state?.position_secs ?? 0;
  const dur = state?.duration_secs ?? cur?.duration_secs ?? 0;
  // flux live (webradio) : pas de durée ni de seek, badge « EN DIRECT » + titre ICY
  const live = state?.is_live ?? false;
  const sleepLeft = alarms?.sleep_remaining_secs ?? null;
  return (
    <footer className="player">
      <div className="now">
        {cur ? (<><div className={`cover ${npOpen ? "active" : ""}`} title={npOpen ? "Fermer le panneau lecture en cours" : "Pochette / Infos / Paroles de la lecture en cours"} onClick={onToggleNp}>{cur.has_cover ? <img src={api.coverUrl(cur.id)} alt="" /> : <Icon name={live ? "radio" : "note"} size={22} />}</div>
          <div className="meta"><div className="title">{cur.title}</div><div className="artist">{live && state?.stream_title ? state.stream_title : cur.artist}</div></div></>) : <div className="meta"><div className="title">Aucune lecture</div></div>}
      </div>
      <div className="center">
        <div className="controls">
          <button className={state?.shuffle ? "active" : ""} title="Aléatoire" onClick={() => act(() => api.shuffle(!state?.shuffle))}><Icon name="shuffle" size={20} /></button>
          <button title="Précédent" onClick={() => act(() => api.prev())}><Icon name="prev" size={22} /></button>
          <button className="play" title={playing ? "Pause" : "Lecture"} onClick={() => act(() => (playing ? api.pause() : api.resume()))}>{playing ? <Icon name="pause" size={18} /> : <Icon name="play" size={18} />}</button>
          <button title="Stop" onClick={() => act(() => api.stop())}><Icon name="stop" size={18} /></button>
          <button title="Suivant" onClick={() => act(() => api.next())}><Icon name="next" size={22} /></button>
          <button className={state?.repeat !== "off" ? "active" : ""} title="Répéter" onClick={() => act(() => api.repeat(state?.repeat === "off" ? "all" : state?.repeat === "all" ? "one" : "off"))}><Icon name="repeat" size={20} /></button>
        </div>
        <div className="seek">
          {live ? (
            <>
              <span className="badge live">{t("EN DIRECT")}</span>
              <span className="num-time" title="Temps écoulé depuis le début du flux">{fmtTime(pos)}</span>
            </>
          ) : (
            <>
              <span>{fmtTime(pos)}</span>
              <input type="range" min={0} max={dur || 0} step={0.1} value={Math.min(pos, dur || 0)} aria-label="Progression"
                onChange={(e) => act(() => api.seek(Number(e.target.value)))} />
              <span>{fmtTime(dur)}</span>
            </>
          )}
        </div>
      </div>
      <div className="right">
        {state?.output_mode === "browser" && <span className="badge" title="Le son sort dans ce navigateur"><Icon name="speaker" size={12} /> Navigateur</span>}
        {sleepLeft != null && <span className="badge" title={`Minuterie de sommeil : pause automatique dans ${fmtTime(sleepLeft)}`}>
          <Icon name="moon" size={12} /> {fmtTime(sleepLeft)}
        </span>}
        {alarms?.wake_time && <span className="badge" title={`Réveil programmé à ${alarms.wake_time} (heure locale)`}>
          <Icon name="alarm" size={12} /> {alarms.wake_time}
        </span>}
        {cur && <button className="icon-btn-out" title="Partager un lien public de la piste en cours" onClick={async () => {
          try {
            const sh = await api.createShare("track", cur.id);
            const url = api.shareUrl(sh.token);
            await navigator.clipboard?.writeText(url).catch(() => {});
            say(`Lien de partage copié : ${url}`);
          } catch (e) { say(e instanceof Error ? e.message : String(e)); }
        }}><Icon name="share" size={20} /></button>}
        <button className="icon-btn-out" title="Mode affichage plein écran « de loin » (écran HDMI — #display)" onClick={onToggleDisplay}>
          <Icon name="display" size={20} />
        </button>
        <button className={`icon-btn-out ${state?.output_mode === "browser" ? "active" : ""}`} title="Choisir la sortie (Jouer ici, Caster…)" onClick={() => { setMenuOpen((v) => !v); refreshDevices(); }}>
          <Icon name="cast" size={20} />
        </button>
        {menuOpen && (
          <>
            <div className="menu-overlay" onClick={() => setMenuOpen(false)} />
            <div className="output-menu">
              <div className="out-title">Lire sur…</div>
              <div className="out-grid">
                <button className={`out-tile ${state?.output_mode === "browser" ? "active" : ""}`}
                  onClick={() => { act(() => api.setOutput(state?.output_mode === "browser" ? "local" : "browser"), state?.output_mode === "browser" ? "Sortie : DAC local" : "Lecture dans ce navigateur"); setMenuOpen(false); }}>
                  <Icon name="speaker" size={30} />
                  <span>{state?.output_mode === "browser" ? "Ce navigateur ✓" : "Jouer ici"}</span>
                </button>
                <button className="out-tile" onClick={() => { say("Regroupement multiroom — prévu en v0.6"); setMenuOpen(false); }}>
                  <Icon name="group" size={30} />
                  <span>Regroupement</span>
                </button>
                <button className="out-tile" onClick={() => { say("Caster (AirPlay/Chromecast) — prévu en v0.6"); setMenuOpen(false); }}>
                  <Icon name="cast" size={30} />
                  <span>Caster</span>
                </button>
                <button className="out-tile" onClick={() => { say("Contrôler d'autres dispositifs — prévu en v0.6"); setMenuOpen(false); }}>
                  <Icon name="devices" size={30} />
                  <span>Autres dispositifs</span>
                </button>
              </div>
            </div>
          </>
        )}
        {cur && <button className="badge badge-btn" title="Signal path — chaîne de traitement appliquée au flux en cours (#77)" onClick={onOpenSignalPath}>
          <Icon name="sliders" size={12} /> Signal path
        </button>}
        {state?.current_format && <span className="badge">{fmtFormat(state.current_format)}</span>}
        {state?.output_backend && <span className="badge" title="Backend de sortie audio actuellement utilisé">{state.output_backend}</span>}
        <select value={state?.audio_device ?? ""} onChange={(e) => act(() => api.selectDevice(e.target.value || null))} aria-label="Périphérique audio"
          title={devices.length === 0 ? "Aucun périphérique de sortie détecté — si vous lancez le binaire Linux/WSL, les DAC USB Windows n'y sont pas visibles : lancez auris.exe sous Windows" : "Périphérique audio"}>
          <option value="">DAC par défaut</option>
          {devices.length === 0 && <option value="" disabled>Aucun périphérique détecté</option>}
          {devices.map((d) => <option key={d.name} value={d.name}>{d.name}{d.is_default ? " ★" : ""}{d.supports_i24 ? " (DoP)" : ""}</option>)}
        </select>
        <button className="icon-btn" title="Rechercher à nouveau les périphériques audio (DAC branché à chaud)" onClick={() => refreshDevices()}><Icon name="refresh" size={16} /></button>
        {state?.volume_mode === "disabled"
          ? <span className="badge" title="Volume géré par le DAC/ampli (bit-perfect)">bit-perfect · vol. sur l'ampli</span>
          : (<>
            <input type="range" min={0} max={state?.max_volume ?? 100} value={state?.muted ? 0 : state?.volume ?? 100} aria-label="Volume"
              onChange={(e) => act(() => api.volume(Number(e.target.value)))} />
            <span className="vol">{state?.muted ? 0 : state?.volume ?? 100}</span>
          </>)}
      </div>
    </footer>
  );
}

/// Panneau latéral « lecture en cours » (style Volumio) : onglets Pochette / Infos / Paroles.
/// Paroles synchronisées (LRC) avec ligne active suivie pendant la lecture.
function NowPlayingPanel({ track, pos, tab, setTab, onClose, isLive = false, streamTitle = null }: {
  track: Track; pos: number; tab: NpTab; setTab: (t: NpTab) => void; onClose: () => void;
  /** Flux live (webradio/podcast) : pas de paroles ni de fiche piste (#90). */
  isLive?: boolean; streamTitle?: string | null;
}) {
  const [lyrics, setLyrics] = useState<LyricsResponse | null>(null);
  const [infos, setInfos] = useState<InfosResponse | null>(null);
  const [err, setErr] = useState<string | null>(null);
  const activeRef = useRef<HTMLDivElement | null>(null);
  // Recharge les paroles + infos à chaque changement de piste (pas pour un flux live).
  useEffect(() => {
    setLyrics(null);
    setInfos(null);
    setErr(null);
    if (isLive) return;
    let dead = false;
    api.lyrics(track.id)
      .then((l) => { if (!dead) setLyrics(l); })
      .catch((e) => { if (!dead) setErr(e instanceof Error ? e.message : String(e)); });
    api.infos(track.id)
      .then((i) => { if (!dead) setInfos(i); })
      .catch(() => { /* les infos sont best-effort */ });
    return () => { dead = true; };
  }, [track.id, isLive]);

  // Un flux live n'a pas d'onglet Paroles : on retombe sur l'onglet radio.
  useEffect(() => {
    if (isLive && tab === "lyrics") setTab("cover");
  }, [isLive, tab, setTab]);

  const synced = lyrics?.synced ?? [];
  // Dernière ligne dont le timestamp est <= la position courante.
  let activeIdx = -1;
  for (let i = 0; i < synced.length; i++) {
    if (synced[i][0] <= pos + 0.05) activeIdx = i;
    else break;
  }
  useEffect(() => {
    activeRef.current?.scrollIntoView({ block: "center" });
  }, [activeIdx]);

  const tabs: [NpTab, string][] = isLive
    ? [["cover", t("Radio")], ["infos", t("Infos")]]
    : [["cover", "Pochette"], ["infos", "Infos"], ["lyrics", "Paroles"]];
  return (
    <aside className="np-panel">
      <div className="np-tabs">
        {tabs.map(([t, label]) => (
          <button key={t} className={`np-tab ${tab === t ? "active" : ""}`} onClick={() => setTab(t)}>{label}</button>
        ))}
        <button className="np-close" title="Fermer" onClick={onClose}><Icon name="close" size={18} /></button>
      </div>
      <div className="np-body">
        {tab === "cover" && (
          <div className="np-cover-wrap">
            <div className={`np-cover ${isLive ? "radio" : ""}`}>
              {isLive
                ? <Icon name="radio" size={72} />
                : track.has_cover ? <img src={api.coverUrl(track.id)} alt="" /> : <Icon name="note" size={64} />}
            </div>
            <div className="np-title">{track.title}</div>
            <div className="np-artist">{isLive ? t("Radio web") : track.artist}</div>
            <div className="np-album">{isLive ? track.uri : track.album}</div>
            <div className="np-badges">
              {isLive && <span className="pill live">{t("EN DIRECT")}</span>}
              {isLive && streamTitle && <span className="badge">{streamTitle}</span>}
              {track.format && <span className="badge">{fmtFormat(track.format)}</span>}
              {!isLive && track.year != null && <span className="badge">{track.year}</span>}
              {!isLive && track.genre && <span className="badge">{track.genre}</span>}
            </div>
          </div>
        )}
        {tab === "infos" && (
          <div className="np-infos">
            {isLive && (
              <>
                <div className="np-h">{t("Radio web")}</div>
                <p className="np-text">{track.title}</p>
                <div className="np-h">{t("Flux")}</div>
                <p className="np-text" style={{ wordBreak: "break-all" }}>{track.uri}</p>
                <div className="np-h">{t("En cours de lecture")}</div>
                <p className="np-text">{streamTitle ?? t("—")}</p>
                <div className="np-badges" style={{ justifyContent: "flex-start", marginTop: 10 }}>
                  <span className="pill live">{t("EN DIRECT")}</span>
                  {track.format && <span className="badge">{fmtFormat(track.format)}</span>}
                </div>
              </>
            )}
            {!isLive && !infos && !err && <div className="np-muted">Chargement…</div>}
            {!isLive && err && <div className="np-muted">{err}</div>}
            {infos && !isLive && (<>
              {infos.bio && (<>
                <div className="np-h">{infos.album_artist}</div>
                <p className="np-text">{infos.bio}</p>
              </>)}
              {infos.album_info && (<>
                <div className="np-h">L'album</div>
                <p className="np-text">
                  {infos.album_info.title} — {infos.album_info.artist}
                  {infos.album_info.year != null ? `, ${infos.album_info.year}` : ""}
                  {` · ${infos.album_info.track_count} pistes · ${fmtTime(infos.album_info.duration_secs)}`}
                </p>
              </>)}
              <div className="np-h">Cette piste</div>
              <p className="np-text">
                {track.title}
                {track.track_number != null ? ` · piste ${track.track_number}${track.disc_number != null ? ` (disque ${track.disc_number})` : ""}` : ""}
                {track.duration_secs != null ? ` · ${fmtTime(track.duration_secs)}` : ""}
                {track.format ? ` · ${fmtFormat(track.format)}` : ""}
              </p>
              {(() => {
                const rows: [string, string | null][] = [
                  [t("Compositeur"), infos.composer],
                  [t("Producteur"), infos.producer],
                  [t("Label"), infos.label ?? infos.album_info?.label ?? null],
                  [t("Éditeur"), infos.publisher ?? infos.album_info?.publisher ?? null],
                  ["ISRC", infos.isrc],
                  ["Copyright", infos.copyright ?? infos.album_info?.copyright ?? null],
                ].filter((r): r is [string, string | null] => !!r[1]);
                if (!rows.length) return null;
                return (<>
                  <div className="np-h">{t("Crédits")}</div>
                  <table className="np-credits"><tbody>
                    {rows.map(([k, v]) => <tr key={k}><td>{k}</td><td>{v}</td></tr>)}
                  </tbody></table>
                </>);
              })()}
              {infos.comment && (<>
                <div className="np-h">Commentaire</div>
                <p className="np-text">{infos.comment}</p>
              </>)}
              {(() => {
                const mb = infos.musicbrainz;
                const utile = mb && (mb.mbid || mb.release || mb.label || mb.rating != null || (mb.tags?.length ?? 0) > 0 || (mb.isrcs?.length ?? 0) > 0);
                if (!utile) return null;
                return (<>
                <div className="np-h">MusicBrainz</div>
                <table className="np-credits"><tbody>
                  {mb.release && <tr><td>Sortie</td><td>
                    {mb.url
                      ? <a href={mb.url} target="_blank" rel="noreferrer">{mb.release}</a>
                      : mb.release}
                    {mb.release_date ? ` (${mb.release_date})` : ""}
                  </td></tr>}
                  {mb.label && <tr><td>Label</td><td>{mb.label}</td></tr>}
                  {mb.isrcs?.length > 0 && <tr><td>ISRC</td><td>{mb.isrcs.join(", ")}</td></tr>}
                  {mb.rating != null && <tr><td>Note</td><td>{mb.rating.toFixed(1)} / 5{mb.rating_count != null ? ` (${mb.rating_count} votes)` : ""}</td></tr>}
                </tbody></table>
                {mb.tags?.length > 0 && (
                  <div className="np-badges" style={{ marginTop: 6 }}>
                    {mb.tags.slice(0, 8).map((tg) => <span key={tg} className="badge">{tg}</span>)}
                  </div>
                )}
              </>);
              })()}
            </>)}
          </div>
        )}
        {tab === "lyrics" && !isLive && (
          <div className="np-lyrics">
            {!lyrics && !err && <div className="np-muted">Chargement…</div>}
            {err && <div className="np-muted">{err}</div>}
            {lyrics && synced.length > 0 && synced.map(([ts, line], i) => (
              <div
                key={`${ts}-${i}`}
                ref={i === activeIdx ? activeRef : undefined}
                className={`line ${i === activeIdx ? "active" : ""}`}
              >{line}</div>
            ))}
            {lyrics && synced.length === 0 && lyrics.plain && (
              <p className="np-text">{lyrics.plain}</p>
            )}
            {lyrics && synced.length === 0 && !lyrics.plain && (
              <div className="np-muted">Aucune parole trouvée pour cette piste.</div>
            )}
            {lyrics && <div className="np-muted np-src">source : {lyrics.source === "tag" ? "tag embarqué" : lyrics.source === "lrclib" ? "lrclib.net" : "aucune"}</div>}
          </div>
        )}
      </div>
    </aside>
  );
}
