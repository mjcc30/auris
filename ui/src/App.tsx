import { useCallback, useEffect, useRef, useState } from "react";

import { api, dopCarrier, fmtBytes, fmtMix, fmtTime, makeSource, type AlarmsInfo, type Album, type AppearanceSettings, type Artist, type ArtistDetail, type AudioDevice, type BrowseEntry, type CopyReport, type DopStatus, type Facets, type FavKind, type Favorites, type FavoritesResolved, type HistoryTrack, type InfosResponse, type LibraryStats, type LyricsResponse, type PinKind, type Pins, type MountInfo, type Packs, type Playlist, type PlaylistDetail, type PlayerState, type RadioEntry, type RadioList, type DirCountry, type DirStation, type DirTag, type SearchResults, type Settings, type ShareLink, type Source, type StorageInfo, type SystemInfo, type Track } from "./api";
import { Icon } from "./Icon";
import { BrowserAudio } from "./browserAudio";
import { loadLocalePack, setLocale, t } from "./i18n";

type View = "home" | "library" | "browse" | "radios" | "queue" | "playlists" | "favorites" | "sources" | "settings";
type SourceKindTab = "local" | "usb" | "smb" | "radio" | "http";
/** Onglets du panneau lecture en cours (style Volumio: Pochette / Infos / Paroles). */
type NpTab = "cover" | "infos" | "lyrics";

function kindIconName(kind: string): string {
  switch (kind) {
    case "usb": return "usb";
    case "smb": return "server";
    case "upnp": return "radio";
    case "radio": return "radio";
    case "http": return "link";
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

  useEffect(() => {
    refresh();
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
  }, [refresh, say]);

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
  const globalSearch = () => { if (searchQ.trim()) { setSearchNonce((n) => n + 1); setView("library"); } };
  const refreshDevices = useCallback(async () => {
    try { setDevices(await api.devices()); } catch { /* inchangé si erreur */ }
  }, []);

  return (
    <div className="app">
      <div className="main">
        <Sidebar view={view} setView={setView} sources={sources} browseSrc={browseSrc} openSource={openSource} system={system} stats={stats} />
        <main className="content">
          <div className="topbar">
            <div className="topbar-title">{view === "home" ? t("Accueil") : view === "library" ? t("Bibliothèque") : view === "browse" ? t("Parcourir") : view === "radios" ? t("Radios") : view === "queue" ? t("File d'attente") : view === "playlists" ? t("Listes de lecture") : view === "favorites" ? t("Favoris") : view === "sources" ? t("Sources") : t("Réglages")}</div>
            <div className="topbar-search">
              <Icon name="search" size={18} />
              <input type="text" placeholder={t("Rechercher un titre, un artiste…")} value={searchQ}
                onChange={(e) => setSearchQ(e.target.value)} onKeyDown={(e) => e.key === "Enter" && globalSearch()} />
              <button className="icon-btn" title={t("Rechercher")} onClick={globalSearch}><Icon name="search" size={18} /></button>
            </div>
          </div>
          {view === "home" && (
            <Home state={state} sources={sources} favs={favs} onToggleFav={toggleFav} pins={pins} onTogglePin={togglePin} playTrack={playTrack} playAll={playAll} openSource={openSource}
              openArtist={(a) => { setPendingArtist(a); setPendingAlbum(null); setView("library"); }}
              openAlbum={(a) => { setPendingAlbum(a); setPendingArtist(null); setView("library"); }} />
          )}
          {view === "library" && (
            <Library playTrack={playTrack} playAll={playAll} say={say} refresh={refresh} searchNonce={searchNonce} searchQ={searchQ}
              favs={favs} onToggleFav={toggleFav} pins={pins} onTogglePin={togglePin}
              pendingArtist={pendingArtist} onPendingArtistDone={() => setPendingArtist(null)}
              pendingAlbum={pendingAlbum} onPendingAlbumDone={() => setPendingAlbum(null)} />
          )}
          {view === "favorites" && (
            <FavoritesView playTrack={playTrack} playAll={playAll} favs={favs} onToggleFav={toggleFav} pins={pins} onTogglePin={togglePin}
              openAlbum={(a) => { setPendingAlbum(a); setPendingArtist(null); setView("library"); }}
              openArtist={(a) => { setPendingArtist(a); setPendingAlbum(null); setView("library"); }} />
          )}
          {view === "browse" && <Browse sources={sources} src={browseSrc} playUri={playUri} act={act} say={say} />}
          {view === "radios" && <Radios state={state} act={act} say={say} pins={pins} onTogglePin={togglePin} />}
          {view === "queue" && <Queue state={state} act={act} say={say} playAll={playAll} />}
          {view === "playlists" && <Playlists act={act} say={say} playAll={playAll} playTrack={playTrack} pins={pins} onTogglePin={togglePin} />}
          {view === "sources" && <Sources sources={sources} refresh={refresh} say={say} />}
          {view === "settings" && <SettingsView settings={settings} updateSettings={updateSettings} say={say} refresh={refresh} packs={packs} />}
        </main>
        {npOpen && state?.current && (
          <NowPlayingPanel
            track={state.current}
            pos={state.position_secs}
            tab={npTab}
            setTab={setNpTab}
            onClose={() => setNpOpen(false)}
          />
        )}
      </div>
      {dsdCarrier != null && dop && (
        <DopBanner carrier={dsdCarrier} dop={dop} deviceName={state?.audio_device ?? null} backend={state?.output_backend ?? null} say={say}
          onChanged={() => api.dopStatus().then(setDop).catch(() => {})} />
      )}
      <PlayerBar state={state} devices={devices} act={act} say={say} alarms={alarms} refreshDevices={refreshDevices} npOpen={npOpen} onToggleNp={() => setNpOpen((v) => !v)} />
      {toast && <div className="toast">{toast}</div>}
    </div>
  );
}

function Sidebar({ view, setView, sources, browseSrc, openSource, system, stats }: {
  view: View; setView: (v: View) => void; sources: Source[];
  browseSrc: string | null; openSource: (id: string) => void; system: SystemInfo | null;
  stats: LibraryStats | null;
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
  const [tracks, setTracks] = useState<Track[]>([]);
  const [artists, setArtists] = useState<Artist[]>([]);
  const [facets, setFacets] = useState<Facets | null>(null);
  const [history, setHistory] = useState<HistoryTrack[]>([]);
  const [facet, setFacet] = useState<Facet | null>(null);
  useEffect(() => {
    api.albums().then(setAlbums).catch(() => {});
    api.tracks().then((t) => setTracks(t.slice(0, 20))).catch(() => {});
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
      <TrackTable tracks={tracks} current={state?.current?.id} favs={favs} onToggleFav={onToggleFav} playTrack={playTrack} playAll={playAll} />
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
                <div className="cover">{a.cover_track_id ? <img src={api.coverUrl(a.cover_track_id)} alt="" /> : <Icon name="note" size={44} />}
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

function Library({ playTrack, playAll, say, refresh, searchNonce, searchQ, pendingArtist, onPendingArtistDone, pendingAlbum, onPendingAlbumDone, favs, onToggleFav, pins, onTogglePin }: { playTrack: (t: Track) => void; playAll: (t: Track[]) => void; say: (m: string) => void; refresh: () => Promise<void>; searchNonce: number; searchQ: string; pendingArtist: { id: string; name: string } | null; onPendingArtistDone: () => void; pendingAlbum: { id: string; title: string; artist: string } | null; onPendingAlbumDone: () => void } & LibProps) {
  const [tab, setTab] = useState<"tracks" | "albums" | "artists">("tracks");
  const [tracks, setTracks] = useState<Track[]>([]);
  const [albums, setAlbums] = useState<Album[]>([]);
  const [artists, setArtists] = useState<Artist[]>([]);
  const [q, setQ] = useState("");
  const [searching, setSearching] = useState(false);
  const [results, setResults] = useState<SearchResults | null>(null);
  const [kind, setKind] = useState<"all" | "tracks" | "albums" | "artists" | "playlists">("all");
  const [srcFilter, setSrcFilter] = useState("");
  const [sources, setSources] = useState<Source[]>([]);
  const [plDetail, setPlDetail] = useState<PlaylistDetail | null>(null);
  const [scanning, setScanning] = useState(false);
  const [albumDetail, setAlbumDetail] = useState<{ id: string; title: string; artist: string } | null>(null);
  const [artistDetail, setArtistDetail] = useState<{ id: string; name: string } | null>(null);
  // Ouverture demandée depuis l'accueil (carte artiste/album) : on ouvre la vue correspondante.
  useEffect(() => {
    if (pendingArtist) { setArtistDetail(pendingArtist); setAlbumDetail(null); onPendingArtistDone(); }
  }, [pendingArtist, onPendingArtistDone]);
  useEffect(() => {
    if (pendingAlbum) { setAlbumDetail(pendingAlbum); setArtistDetail(null); onPendingAlbumDone(); }
  }, [pendingAlbum, onPendingAlbumDone]);
  useEffect(() => {
    api.tracks().then(setTracks).catch(() => {});
    api.albums().then(setAlbums).catch(() => {});
    api.artists().then(setArtists).catch(() => {});
    api.sources().then(setSources).catch(() => {});
  }, []);
  const runSearch = (query: string, src: string) => {
    if (!query.trim()) {
      setSearching(false);
      setResults(null);
      api.tracks().then(setTracks).catch(() => {});
      return;
    }
    setSearching(true);
    api.search(query, src || undefined).then(setResults).catch(() => {});
  };
  useEffect(() => {
    if (searchNonce > 0) {
      setQ(searchQ);
      setKind("all");
      runSearch(searchQ, srcFilter);
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
  const doSearch = () => runSearch(q, srcFilter);
  const clearSearch = () => {
    setSearching(false);
    setResults(null);
    setQ("");
    api.tracks().then(setTracks).catch(() => {});
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
      <div className="searchbar">
        <div className="input-wrap"><Icon name="search" size={18} /><input type="text" placeholder={t("Rechercher un titre, un album, un artiste…")} value={q}
          onChange={(e) => { const v = e.target.value; setQ(v); if (v.trim()) setSearching(true); else clearSearch(); }}
          onKeyDown={(e) => e.key === "Enter" && doSearch()} /></div>
        {searching && <button className="btn ghost" onClick={clearSearch}>{t("Effacer")}</button>}
        <button className="btn" disabled={scanning} onClick={scan}>{scanning ? t("Scan…") : t("Scanner la source")}</button>
      </div>
      {searching ? (
        results ? (
        <div>
          <div className="chips" style={{ marginTop: 0, marginBottom: 4 }}>
            {([["all", t("Tous")], ["tracks", `${t("Titres")} (${results.tracks.total})`], ["albums", `${t("Albums")} (${results.albums.total})`], ["artists", `${t("Artistes")} (${results.artists.total})`], ["playlists", `${t("Listes")} (${results.playlists.total})`]] as const).map(([k, label]) => (
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
          {results.tracks.total + results.albums.total + results.artists.total + results.playlists.total === 0 && (
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
                    <div className="cover">{a.cover_track_id ? <img src={api.coverUrl(a.cover_track_id)} alt="" /> : <Icon name="note" size={44} />}
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
      <div className="seg">
        {([["tracks", "Titres"], ["albums", "Albums"], ["artists", "Artistes"]] as const).map(([t, label]) => (
          <button key={t} className={`seg-btn ${tab === t ? "active" : ""}`} onClick={() => setTab(t)}>{label}</button>
        ))}
      </div>
      {tab === "tracks" && <TrackTable tracks={tracks} playTrack={playTrack} playAll={playAll} favs={favs} onToggleFav={onToggleFav} />}
      {tab === "albums" && (
        <div className="grid">
          {albums.map((a) => (
            <div key={a.id} className="card" onClick={() => setAlbumDetail({ id: a.id, title: a.title, artist: a.artist })}>
              <div className="cover">{a.cover_track_id ? <img src={api.coverUrl(a.cover_track_id)} alt="" /> : <Icon name="note" size={44} />}
                <PinButton on={pins.albums.includes(a.id)} onToggle={() => onTogglePin("album", a.id)} />
                <FavHeart on={favs.albums.includes(a.id)} onToggle={() => onToggleFav("album", a.id)} />
              </div>
              <div className="t">{a.title}</div>
              <div className="s">{a.artist} · {a.track_count} titres · {fmtTime(a.duration_secs)}</div>
            </div>
          ))}
        </div>
      )}
      {tab === "artists" && (
        <table className="tracks"><tbody>
          {artists.map((a) => <tr key={a.id} className="link" onClick={() => setArtistDetail({ id: a.id, name: a.name })}><td>{a.name}</td><td className="meta"><FavHeart on={favs.artists.includes(a.id)} onToggle={() => onToggleFav("artist", a.id)} /> {cl(a.album_count, "album", "albums")}</td></tr>)}
        </tbody></table>
      )}
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
                <div className="cover">{a.cover_track_id ? <img src={api.coverUrl(a.cover_track_id)} alt="" /> : <Icon name="note" size={44} />}
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
  useEffect(() => {
    fetch(`/api/library/albums/${encodeURIComponent(id)}`).then((r) => r.json()).then((j) => {
      setTracks(j.tracks ?? []);
      setCover(j.cover_url ?? null);
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
      <TrackTable tracks={tracks} playTrack={playTrack} playAll={playAll} favs={favs} onToggleFav={onToggleFav} albumCover={cover} />
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
                    <div className="cover">{a.cover_track_id ? <img src={api.coverUrl(a.cover_track_id)} alt="" /> : <Icon name="note" size={44} />}
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

function TrackTable({ tracks, current, playTrack, playAll, albumCover, favs, onToggleFav }: { tracks: Track[]; current?: string | null; playTrack: (t: Track) => void; playAll: (t: Track[]) => void; albumCover?: string | null; favs?: Favorites; onToggleFav?: (kind: FavKind, id: string) => void }) {
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
            <td className="num">{favs && onToggleFav && <FavHeart on={favs.tracks.includes(tr.id)} onToggle={() => onToggleFav("track", tr.id)} />}</td>
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

function Browse({ sources, src, playUri, act, say }: { sources: Source[]; src: string | null; playUri: (u: string) => void; act: (fn: () => Promise<unknown>, ok?: string) => Promise<void>; say: (m: string) => void }) {
  const [path, setPath] = useState<string[]>([]);
  const [entries, setEntries] = useState<BrowseEntry[]>([]);
  const [loading, setLoading] = useState(false);
  const [grid, setGrid] = useState(true);
  const [menu, setMenu] = useState<MenuState>(null);
  const [menuSub, setMenuSub] = useState<null | "playlists">(null);
  const [playlists, setPlaylists] = useState<Playlist[]>([]);
  const [copyTarget, setCopyTarget] = useState<BrowseEntry | null>(null);
  const id = src ?? sources[0]?.id ?? null;

  useEffect(() => { setPath([]); }, [id]);
  useEffect(() => {
    if (!id) return;
    const uri = `src://${id}/${path.join("/")}`;
    setLoading(true);
    api.browse(uri).then(setEntries).catch((e) => say(e instanceof Error ? e.message : String(e))).finally(() => setLoading(false));
    setMenu(null);
    setMenuSub(null);
  }, [id, path, say]);

  if (!id) return <div className="empty">Aucune source configurée — ajoutez-en une dans « Sources &amp; réglages ».</div>;
  const source = sources.find((s) => s.id === id);
  const curUri = `src://${id}/${path.join("/")}`;

  // Un clic simple ouvre un dossier ou lance un fichier audio (plus de double clic).
  const openEntry = (e: BrowseEntry) => {
    if (e.is_dir) setPath([...path, e.name]);
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
              <button className="crumb-btn" onClick={() => setPath(path.slice(0, i + 1))}>{p}</button>
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
      <h1><Icon name={kindIconName(source?.kind ?? "local")} size={28} /> {source?.name ?? id}{path.length > 0 && <span className="crumb-sep"> — {path.join(" / ")}</span>}</h1>
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
                <div className="ctx-item" onClick={() => { act(async () => { await api.scan(id); }, "Mise à jour du dossier lancée"); closeMenu(); }}><Icon name="refresh" size={16} /> Mettre à jour le dossier</div>
                <div className="ctx-item" onClick={() => { window.location.href = api.exportUrl(menu.entry.uri, menu.entry.name.replace(/\.[^.]+$/, "")); closeMenu(); }}><Icon name="download" size={16} /> Télécharger (ZIP)</div>
                <div className="ctx-item" onClick={() => { setCopyTarget(menu.entry); closeMenu(); }}><Icon name="copy" size={16} /> Copier vers une autre source…</div>
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

type DirData = DirTag[] | DirCountry[] | DirStation[];

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

  const reload = useCallback(async () => {
    try { setList(await api.radios()); }
    catch (e) { say(e instanceof Error ? e.message : String(e)); }
  }, [say]);

  useEffect(() => { reload(); }, [reload]);

  // chargement de la sous-vue annuaire (top/popular/genres/pays/recherche…)
  useEffect(() => {
    if (view === "root" || view === "pinned" || view === "fav" || view === "builtin" || view === "sources") return;
    let cancelled = false;
    setDirLoading(true);
    const load = async () => {
      try {
        let data: DirData;
        if (view === "genres") data = await api.radiosDirGenres();
        else if (view === "countries") data = await api.radiosDirCountries();
        else if (view === "top") data = await api.radiosDirStations("top");
        else if (view === "popular") data = await api.radiosDirStations("popular");
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

  const viewTitle = () => {
    if (view === "pinned") return t("Radios épinglées");
    if (view === "fav") return t("Mes radios");
    if (view === "builtin") return t("Radios par défaut");
    if (view === "sources") return t("Sources radio");
    if (view === "top") return t("Top radios");
    if (view === "popular") return t("Radios populaires");
    if (view === "genres") return t("Par genre");
    if (view === "countries") return t("Par pays");
    if (view.startsWith("genre:")) return t("Genre : {name}", { name: view.slice(6) });
    if (view.startsWith("country:")) return t("Pays : {name}", { name: view.slice(8) });
    if (view.startsWith("search:")) return t("Recherche : {q}", { q: view.slice(7) });
    return t("Radios");
  };

  const tiles = [
    { id: "pinned", icon: "pin", label: t("Épinglées"), sub: t("{count} radio(s)", { count: pins.radios.length }) },
    { id: "fav", icon: "heart", label: t("Mes radios"), sub: t("{count} radio(s)", { count: list?.favorites.length ?? 0 }) },
    { id: "builtin", icon: "radio", label: t("Radios par défaut"), sub: t("{count} radio(s)", { count: list?.builtin.length ?? 0 }) },
    { id: "sources", icon: "server", label: t("Sources radio"), sub: t("{count} source(s)", { count: list?.sources.length ?? 0 }) },
    { id: "top", icon: "star", label: t("Top radios"), sub: t("les mieux notées") },
    { id: "popular", icon: "flame", label: t("Radios populaires"), sub: t("les plus écoutées") },
    { id: "genres", icon: "note", label: t("Par genre"), sub: t("rock, jazz, classique…") },
    { id: "countries", icon: "globe", label: t("Par pays"), sub: t("du monde entier") },
    { id: "__search", icon: "search", label: t("Rechercher"), sub: t("par nom de radio") },
    { id: "__add", icon: "plus", label: t("Ajouter une radio"), sub: t("URL de flux personnalisée") },
  ];

  const onTile = (id: string) => {
    if (id === "__search") {
      const q = window.prompt(t("Rechercher une radio (nom) :"));
      if (q && q.trim()) setView(`search:${q.trim()}`);
    } else if (id === "__add") {
      setShowAdd((v) => !v);
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
    // ne rend les données de l'annuaire que si elles correspondent à la vue courante
    const data = dir && dir.view === view ? dir.data : null;
    if (dirLoading || !data) return <div className="empty">{t("Chargement de l'annuaire…")}</div>;
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
            {queue.map((t, i) => (
              <tr key={`${t.id}-${i}`} className={idx === i ? "playing" : ""} onClick={() => playAll(queue.slice(i))}>
                <td className="num">{idx === i ? <Icon name="play" size={12} filled /> : i + 1}</td>
                <td>{t.title}</td><td>{t.artist}</td><td>{t.album}</td>
                <td className="num-time">{t.duration_secs != null ? fmtTime(t.duration_secs) : "—"}</td>
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

function Playlists({ act, say, playAll, playTrack, pins, onTogglePin }: { act: (fn: () => Promise<unknown>, ok?: string) => Promise<void>; say: (m: string) => void; playAll: (t: Track[]) => void; playTrack: (t: Track) => void; pins: Pins; onTogglePin: (kind: PinKind, id: string) => void }) {
  const [pls, setPls] = useState<{ id: string; name: string; track_ids: string[]; created_at: number }[]>([]);
  const [detail, setDetail] = useState<PlaylistDetail | null>(null);
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
        <h1>{detail.playlist.name}</h1>
        <TrackTable tracks={detail.tracks} playTrack={playTrack} playAll={playAll} />
      </div>
    );
  }
  return (
    <div>
      {pls.length === 0 && <div className="empty">Aucune liste — enregistrez la file d'attente pour en créer une.</div>}
      <div className="grid">
        {[...pls].sort((a, b) => (pins.playlists.includes(b.id) ? 1 : 0) - (pins.playlists.includes(a.id) ? 1 : 0)).map((p) => (
          <div key={p.id} className="card" onClick={() => api.playlist(p.id).then(setDetail).catch((e) => say(String(e)))}>
            <div className="cover"><Icon name="playlist" size={44} />
              <PinButton on={pins.playlists.includes(p.id)} onToggle={() => onTogglePin("playlist", p.id)} />
            </div>
            <div className="t">{p.name}</div>
            <div className="s">{p.track_ids.length} titre(s)</div>
          </div>
        ))}
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
  const [mounts, setMounts] = useState<MountInfo[]>([]);
  const [detecting, setDetecting] = useState(false);
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

  const kinds: [SourceKindTab, string][] = [["local", "Dossier local"], ["usb", "USB"], ["smb", "NAS (SMB/CIFS)"], ["radio", "Radio"], ["http", "URL HTTP"]];

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

function SettingsView({ settings, updateSettings, say, refresh, packs }: {
  settings: Settings | null;
  updateSettings: (patch: (s: Settings) => Settings, ok?: string) => Promise<void>;
  say: (m: string) => void;
  refresh: () => Promise<void>;
  packs: Packs | null;
}) {
  const [q, setQ] = useState("");
  const [storage, setStorage] = useState<StorageInfo | null>(null);
  const [logs, setLogs] = useState<string | null>(null);
  const [busy, setBusy] = useState("");
  const [shares, setShares] = useState<ShareLink[]>([]);
  const loadShares = useCallback(() => { api.listShares().then(setShares).catch(() => {}); }, []);
  useEffect(() => { loadShares(); }, [loadShares]);

  // Statut DoP/WASAPI des périphériques (Windows uniquement).
  const [dop, setDop] = useState<DopStatus | null>(null);
  const [dopRate, setDopRate] = useState(176400);
  const [dopBusy, setDopBusy] = useState(false);
  const loadDop = useCallback(() => { api.dopStatus().then(setDop).catch(() => {}); }, []);
  useEffect(() => { loadDop(); }, [loadDop]);

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

  const p = settings.playback, ap = settings.appearance, sy = settings.system, nw = settings.network, so = settings.sources;

  const sections: { title: string; rows: React.ReactNode[] }[] = [
    {
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
      title: t("Réseau"),
      rows: [
        show("Nom d'hôte", "Nom d'hôte souhaité pour la machine (Linux uniquement)") && (
          <SetRow key="hostname" label="Nom d'hôte" desc="Appliqué au mieux selon l'OS (Linux uniquement)">
            <input type="text" value={nw.hostname ?? ""} placeholder="auris.local" style={{ width: 220 }}
              onChange={(e) => setNetwork("hostname", e.target.value.trim() || null)} />
          </SetRow>
        ),
      ].filter(Boolean) as React.ReactNode[],
    },
    {
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
      title: t("Partage"),
      rows: [
        show("Liens de partage", "URL publiques /share/<token> pour écouter une piste, un album ou une playlist depuis un autre appareil") && (
          <SetRow key="shares" label="Liens de partage" desc={shares.length ? `${shares.length} lien(s) actif(s)` : "aucun lien actif"}>
            <button className="btn ghost sm" onClick={loadShares}><Icon name="refresh" size={16} /> Actualiser</button>
          </SetRow>
        ),
      ].filter(Boolean) as React.ReactNode[],
    },
  ].map((sec) => ({ ...sec, rows: sec.rows.filter(Boolean) }));

  const visible = sections.filter((s) => s.rows.length > 0);

  return (
    <div className="settings">
      <div className="settings-search">
        <Icon name="search" size={18} />
        <input type="text" placeholder={t("Rechercher dans les réglages…")} value={q} onChange={(e) => setQ(e.target.value)} />
      </div>
      {visible.length === 0 && <div className="empty">{t("Aucun réglage ne correspond à « {q} ».", { q })}</div>}
      {visible.map((sec) => (
        <section key={sec.title} className="set-card">
          <h2>{sec.title}</h2>
          {sec.rows}
        </section>
      ))}
      {show("DoP", "Régler le DAC pour la lecture DSD (fichiers .dsf) en DoP") && dop?.windows && (
        <section className="set-card">
          <h2>DoP — lecture DSD sur DAC USB (Windows)</h2>
          {(() => {
            const devName = dop.current_device;
            const dev = dop.devices.find((d) => devName != null && sameDevice(d.name, devName)) ?? dop.devices[0] ?? null;
            const mix = dev?.mix ?? null;
            const def = dev?.default_format ?? null;
            const ok = mix != null && !mix.is_float && mix.rate === dopRate;
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
      )}
      {show("Liens de partage") && (
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
    </div>
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
  // RAS si le mix est déjà un PCM entier à la porteuse ; sinon le flux DoP
  // sera rééchantillonné (bruit) ou refusé par le pilote.
  if (!dop.windows || !mix || (!mix.is_float && mix.rate === carrier)) return null;
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

function PlayerBar({ state, devices, act, say, alarms, refreshDevices, npOpen, onToggleNp }: { state: PlayerState | null; devices: AudioDevice[]; act: (fn: () => Promise<unknown>, ok?: string) => Promise<void>; say: (m: string) => void; alarms: AlarmsInfo | null; refreshDevices: () => Promise<void>; npOpen: boolean; onToggleNp: () => void }) {
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
function NowPlayingPanel({ track, pos, tab, setTab, onClose }: {
  track: Track; pos: number; tab: NpTab; setTab: (t: NpTab) => void; onClose: () => void;
}) {
  const [lyrics, setLyrics] = useState<LyricsResponse | null>(null);
  const [infos, setInfos] = useState<InfosResponse | null>(null);
  const [err, setErr] = useState<string | null>(null);
  const activeRef = useRef<HTMLDivElement | null>(null);

  // Recharge les paroles + infos à chaque changement de piste.
  useEffect(() => {
    setLyrics(null);
    setInfos(null);
    setErr(null);
    let dead = false;
    api.lyrics(track.id)
      .then((l) => { if (!dead) setLyrics(l); })
      .catch((e) => { if (!dead) setErr(e instanceof Error ? e.message : String(e)); });
    api.infos(track.id)
      .then((i) => { if (!dead) setInfos(i); })
      .catch(() => { /* les infos sont best-effort */ });
    return () => { dead = true; };
  }, [track.id]);

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

  const tabs: [NpTab, string][] = [["cover", "Pochette"], ["infos", "Infos"], ["lyrics", "Paroles"]];
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
            <div className="np-cover">
              {track.has_cover ? <img src={api.coverUrl(track.id)} alt="" /> : <Icon name="note" size={64} />}
            </div>
            <div className="np-title">{track.title}</div>
            <div className="np-artist">{track.artist}</div>
            <div className="np-album">{track.album}</div>
            <div className="np-badges">
              {track.format && <span className="badge">{fmtFormat(track.format)}</span>}
              {track.year != null && <span className="badge">{track.year}</span>}
              {track.genre && <span className="badge">{track.genre}</span>}
            </div>
          </div>
        )}
        {tab === "infos" && (
          <div className="np-infos">
            {!infos && !err && <div className="np-muted">Chargement…</div>}
            {err && <div className="np-muted">{err}</div>}
            {infos && (<>
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
              {infos.comment && (<>
                <div className="np-h">Commentaire</div>
                <p className="np-text">{infos.comment}</p>
              </>)}
            </>)}
          </div>
        )}
        {tab === "lyrics" && (
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
