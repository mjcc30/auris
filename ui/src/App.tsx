import { useCallback, useEffect, useRef, useState } from "react";

import { api, fmtBytes, fmtTime, makeSource, type Album, type AppearanceSettings, type AudioDevice, type BrowseEntry, type CopyReport, type MountInfo, type PlaylistDetail, type PlayerState, type Settings, type Source, type StorageInfo, type SystemInfo, type Track } from "./api";
import { Icon } from "./Icon";
import { BrowserAudio } from "./browserAudio";

type View = "home" | "library" | "browse" | "queue" | "playlists" | "sources" | "settings";
type SourceKindTab = "local" | "usb" | "smb" | "radio" | "http";

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

/// Assombrit/éclaircit une couleur `#RRGGBB` (amt en [-255, 255]).
function shade(hex: string, amt: number): string {
  const m = /^#?([0-9a-f]{6})$/i.exec(hex.trim());
  if (!m) return hex;
  const n = parseInt(m[1], 16);
  const f = (x: number) => Math.max(0, Math.min(255, x + amt));
  const r = f((n >> 16) & 255), g = f((n >> 8) & 255), b = f(n & 255);
  return `#${((r << 16) | (g << 8) | b).toString(16).padStart(6, "0")}`;
}

/// Applique thème / accent / fond au document (persisté côté backend).
function applyAppearance(a: AppearanceSettings) {
  const root = document.documentElement;
  const theme = a.theme === "system"
    ? (window.matchMedia("(prefers-color-scheme: light)").matches ? "light" : "dark")
    : a.theme;
  root.dataset.theme = theme;
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
  const [browseSrc, setBrowseSrc] = useState<string | null>(null);
  const [toast, setToast] = useState<string | null>(null);
  const [searchNonce, setSearchNonce] = useState(0);
  const [searchQ, setSearchQ] = useState("");
  const toastTimer = useRef<number | null>(null);
  const browserAudio = useRef(new BrowserAudio());

  const say = useCallback((msg: string) => {
    setToast(msg);
    if (toastTimer.current) window.clearTimeout(toastTimer.current);
    toastTimer.current = window.setTimeout(() => setToast(null), 4500);
  }, []);

  const refresh = useCallback(async () => {
    try {
      const [s, srcs, devs, sys, st] = await Promise.all([api.state(), api.sources(), api.devices(), api.system(), api.settings()]);
      setState(s); setSources(srcs); setDevices(devs); setSystem(sys); setSettings(st);
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
        else if (ev?.type) refresh();
        else setState(ev as PlayerState);
      } catch { refresh(); }
    };
    const iv = setInterval(refresh, 10000);
    return () => { ws.close(); clearInterval(iv); };
  }, [refresh]);

  // Applique l'apparence (thème, accent, fond) au document à chaque changement.
  const appearance = settings?.appearance;
  useEffect(() => {
    if (appearance) applyAppearance(appearance);
    // Dépendances sur les champs primitifs : évite de réécrire les styles du document à chaque message WS.
  }, [appearance?.theme, appearance?.accent, appearance?.background]);

  // « Jouer ici » : démarre/arrête le lecteur WebAudio selon la sortie active.
  useEffect(() => {
    if (state?.output_mode === "browser") browserAudio.current.start();
    else browserAudio.current.stop();
  }, [state?.output_mode]);

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
  const openSource = (id: string) => { setBrowseSrc(id); setView("browse"); };
  const globalSearch = () => { if (searchQ.trim()) { setSearchNonce((n) => n + 1); setView("library"); } };

  return (
    <div className="app">
      <div className="main">
        <Sidebar view={view} setView={setView} sources={sources} browseSrc={browseSrc} openSource={openSource} system={system} />
        <main className="content">
          <div className="topbar">
            <div className="topbar-title">{view === "home" ? "Accueil" : view === "library" ? "Bibliothèque" : view === "browse" ? "Parcourir" : view === "queue" ? "File d'attente" : view === "playlists" ? "Listes de lecture" : view === "sources" ? "Sources" : "Réglages"}</div>
            <div className="topbar-search">
              <Icon name="search" size={18} />
              <input type="text" placeholder="Rechercher un titre, un artiste…" value={searchQ}
                onChange={(e) => setSearchQ(e.target.value)} onKeyDown={(e) => e.key === "Enter" && globalSearch()} />
            </div>
          </div>
          {view === "home" && <Home state={state} sources={sources} playTrack={playTrack} playAll={playAll} openSource={openSource} />}
          {view === "library" && <Library playTrack={playTrack} playAll={playAll} say={say} refresh={refresh} searchNonce={searchNonce} searchQ={searchQ} />}
          {view === "browse" && <Browse sources={sources} src={browseSrc} playUri={playUri} act={act} say={say} />}
          {view === "queue" && <Queue state={state} act={act} say={say} playAll={playAll} />}
          {view === "playlists" && <Playlists act={act} say={say} playAll={playAll} playTrack={playTrack} />}
          {view === "sources" && <Sources sources={sources} refresh={refresh} say={say} />}
          {view === "settings" && <SettingsView settings={settings} updateSettings={updateSettings} say={say} refresh={refresh} />}
        </main>
      </div>
      <PlayerBar state={state} devices={devices} act={act} say={say} />
      {toast && <div className="toast">{toast}</div>}
    </div>
  );
}

function Sidebar({ view, setView, sources, browseSrc, openSource, system }: {
  view: View; setView: (v: View) => void; sources: Source[];
  browseSrc: string | null; openSource: (id: string) => void; system: SystemInfo | null;
}) {
  const nav: [View, string, string][] = [
    ["home", "Accueil", "home"],
    ["library", "Bibliothèque", "note"],
    ["browse", "Parcourir", "folder"],
    ["queue", "File d'attente", "queue"],
    ["playlists", "Listes de lecture", "playlist"],
    ["sources", "Sources", "server"],
    ["settings", "Réglages", "gear"],
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
      <div className="sources-label">Vos sources</div>
      <div className="sources-list">
        {sources.map((s) => (
          <div key={s.id} className={`src-item ${view === "browse" && browseSrc === s.id ? "active" : ""}`} onClick={() => openSource(s.id)}>
            <span className="src-name"><Icon name={kindIconName(s.kind)} size={18} /> {s.name}</span>
            <span className={`dot ${s.enabled ? "on" : ""}`} />
          </div>
        ))}
      </div>
      {system && <div className="sysinfo">v{system.version} · {system.track_count} titres</div>}
    </nav>
  );
}

function Home({ state, sources, playTrack, playAll, openSource }: { state: PlayerState | null; sources: Source[]; playTrack: (t: Track) => void; playAll: (t: Track[]) => void; openSource: (id: string) => void }) {
  const [albums, setAlbums] = useState<Album[]>([]);
  const [tracks, setTracks] = useState<Track[]>([]);
  useEffect(() => {
    api.albums().then(setAlbums).catch(() => {});
    api.tracks().then((t) => setTracks(t.slice(0, 20))).catch(() => {});
  }, []);
  return (
    <div>
      {state?.current && (
        <div>
          <h2>En lecture</h2>
          <div className="grid">
            <div className="card" onClick={() => playTrack(state.current!)}>
              <div className="cover">{state.current.has_cover ? <img src={api.coverUrl(state.current.id)} alt="" /> : <Icon name="note" size={44} />}</div>
              <div className="t">{state.current.title}</div>
              <div className="s">{state.current.artist} — {state.current.album}</div>
            </div>
          </div>
        </div>
      )}
      <h2>Vos sources</h2>
      <div className="grid">
        {sources.map((s) => (
          <div key={s.id} className="card" onClick={() => openSource(s.id)}>
            <div className="cover"><Icon name={kindIconName(s.kind)} size={44} /></div>
            <div className="t">{s.name}</div>
            <div className="s">{kindLabel(s.kind)}{s.enabled ? "" : " · désactivée"}</div>
          </div>
        ))}
        {sources.length === 0 && <div className="empty">Aucune source — ajoutez-en une dans « Sources &amp; réglages ».</div>}
      </div>
      <h2>Albums récents</h2>
      <div className="grid">
        {albums.slice(0, 12).map((a) => (
          <div key={a.id} className="card">
            <div className="cover">{a.cover_track_id ? <img src={api.coverUrl(a.cover_track_id)} alt="" /> : <Icon name="note" size={44} />}</div>
            <div className="t">{a.title}</div>
            <div className="s">{a.artist}{a.year ? ` · ${a.year}` : ""}</div>
          </div>
        ))}
        {albums.length === 0 && <div className="empty">Scannez une source pour remplir la bibliothèque.</div>}
      </div>
      <h2>Titres</h2>
      <TrackTable tracks={tracks} current={state?.current?.id} playTrack={playTrack} playAll={playAll} />
    </div>
  );
}

function Library({ playTrack, playAll, say, refresh, searchNonce, searchQ }: { playTrack: (t: Track) => void; playAll: (t: Track[]) => void; say: (m: string) => void; refresh: () => Promise<void>; searchNonce: number; searchQ: string }) {
  const [tab, setTab] = useState<"tracks" | "albums" | "artists">("tracks");
  const [tracks, setTracks] = useState<Track[]>([]);
  const [albums, setAlbums] = useState<Album[]>([]);
  const [artists, setArtists] = useState<{ id: string; name: string; album_count: number }[]>([]);
  const [q, setQ] = useState("");
  const [scanning, setScanning] = useState(false);
  const [albumDetail, setAlbumDetail] = useState<{ id: string; title: string; artist: string } | null>(null);
  useEffect(() => {
    api.tracks().then(setTracks).catch(() => {});
    api.albums().then(setAlbums).catch(() => {});
    api.artists().then(setArtists).catch(() => {});
  }, []);
  useEffect(() => {
    if (searchNonce > 0) { setQ(searchQ); setTab("tracks"); api.search(searchQ).then(setTracks).catch(() => {}); }
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [searchNonce]);
  const doSearch = async () => {
    if (!q.trim()) { api.tracks().then(setTracks); return; }
    setTracks(await api.search(q));
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
    return <AlbumView id={albumDetail.id} title={albumDetail.title} artist={albumDetail.artist} playAll={playAll} playTrack={playTrack} onBack={() => setAlbumDetail(null)} />;
  }
  return (
    <div>
      <div className="searchbar">
        <div className="input-wrap"><Icon name="search" size={18} /><input type="text" placeholder="Rechercher…" value={q}
          onChange={(e) => setQ(e.target.value)} onKeyDown={(e) => e.key === "Enter" && doSearch()} /></div>
        <button className="btn ghost" onClick={doSearch}>Rechercher</button>
        <button className="btn" disabled={scanning} onClick={scan}>{scanning ? "Scan…" : "Scanner la source"}</button>
      </div>
      <div className="seg">
        {([["tracks", "Titres"], ["albums", "Albums"], ["artists", "Artistes"]] as const).map(([t, label]) => (
          <button key={t} className={`seg-btn ${tab === t ? "active" : ""}`} onClick={() => setTab(t)}>{label}</button>
        ))}
      </div>
      {tab === "tracks" && <TrackTable tracks={tracks} playTrack={playTrack} playAll={playAll} />}
      {tab === "albums" && (
        <div className="grid">
          {albums.map((a) => (
            <div key={a.id} className="card" onClick={() => setAlbumDetail({ id: a.id, title: a.title, artist: a.artist })}>
              <div className="cover">{a.cover_track_id ? <img src={api.coverUrl(a.cover_track_id)} alt="" /> : <Icon name="note" size={44} />}</div>
              <div className="t">{a.title}</div>
              <div className="s">{a.artist} · {a.track_count} titres · {fmtTime(a.duration_secs)}</div>
            </div>
          ))}
        </div>
      )}
      {tab === "artists" && (
        <table className="tracks"><tbody>
          {artists.map((a) => <tr key={a.id}><td>{a.name}</td><td className="meta">{a.album_count} album(s)</td></tr>)}
        </tbody></table>
      )}
    </div>
  );
}

function AlbumView({ id, title, artist, playAll, playTrack, onBack }: { id: string; title: string; artist: string; playAll: (t: Track[]) => void; playTrack: (t: Track) => void; onBack: () => void }) {
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
      </div>
      <h1>{title}</h1>
      <h2 style={{ marginTop: 0 }}>{artist}</h2>
      <TrackTable tracks={tracks} playTrack={playTrack} playAll={playAll} albumCover={cover} />
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

function TrackTable({ tracks, current, playTrack, playAll, albumCover }: { tracks: Track[]; current?: string | null; playTrack: (t: Track) => void; playAll: (t: Track[]) => void; albumCover?: string | null }) {
  if (tracks.length === 0) return <div className="empty">Aucun titre.</div>;
  return (
    <table className="tracks">
      <thead><tr><th>#</th><th>Titre</th><th>Artiste</th><th>Album</th><th>Format</th><th>Durée</th></tr></thead>
      <tbody>
        {tracks.map((t, i) => (
          <tr key={t.id} className={current === t.id ? "playing" : ""} onDoubleClick={() => playTrack(t)} onClick={() => playAll(tracks.slice(i))}>
            <td className="num">{albumCover && i === 0 ? <img src={albumCover} alt="" style={{ width: 28, height: 28, borderRadius: 4, verticalAlign: "middle" }} /> : i + 1}</td>
            <td>{t.title}</td><td>{t.artist}</td><td>{t.album}</td>
            <td><span className="badge">{t.format ? fmtFormat(t.format) : ""}</span></td>
            <td className="num-time">{t.duration_secs != null ? fmtTime(t.duration_secs) : "—"}</td>
          </tr>
        ))}
      </tbody>
    </table>
  );
}

// ---------- Parcourir ----------

type MenuState = { x: number; y: number; entry: BrowseEntry } | null;

function Browse({ sources, src, playUri, act, say }: { sources: Source[]; src: string | null; playUri: (u: string) => void; act: (fn: () => Promise<unknown>, ok?: string) => Promise<void>; say: (m: string) => void }) {
  const [path, setPath] = useState<string[]>([]);
  const [entries, setEntries] = useState<BrowseEntry[]>([]);
  const [loading, setLoading] = useState(false);
  const [grid, setGrid] = useState(true);
  const [menu, setMenu] = useState<MenuState>(null);
  const [copyTarget, setCopyTarget] = useState<BrowseEntry | null>(null);
  const id = src ?? sources[0]?.id ?? null;

  useEffect(() => { setPath([]); }, [id]);
  useEffect(() => {
    if (!id) return;
    const uri = `src://${id}/${path.join("/")}`;
    setLoading(true);
    api.browse(uri).then(setEntries).catch((e) => say(e instanceof Error ? e.message : String(e))).finally(() => setLoading(false));
    setMenu(null);
  }, [id, path, say]);

  if (!id) return <div className="empty">Aucune source configurée — ajoutez-en une dans « Sources &amp; réglages ».</div>;
  const source = sources.find((s) => s.id === id);
  const curUri = `src://${id}/${path.join("/")}`;

  const openMenu = (e: React.MouseEvent, entry: BrowseEntry) => {
    e.preventDefault();
    e.stopPropagation();
    setMenu({ x: e.clientX, y: e.clientY, entry });
  };

  return (
    <div>
      <div className="toolbar">
        <div className="crumb">
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
            <div key={e.uri} className="card" onDoubleClick={() => (e.is_dir ? setPath([...path, e.name]) : playUri(e.uri))}
              onClick={() => { if (e.is_audio) playUri(e.uri); }}
              onContextMenu={(ev) => openMenu(ev, e)} title={e.name}>
              <div className="cover"><Icon name={e.is_dir ? "folder" : "note"} size={44} /></div>
              <div className="t">{e.name}</div>
              <div className="s">{e.is_dir ? "dossier" : `${e.is_audio ? "audio" : "fichier"}${e.size ? ` · ${fmtBytes(e.size)}` : ""}`}</div>
            </div>
          ))}
        </div>
      )}
      {!loading && !grid && entries.map((e) => (
        <div key={e.uri} className="browse-row" onDoubleClick={() => (e.is_dir ? setPath([...path, e.name]) : playUri(e.uri))}
          onClick={() => e.is_audio && playUri(e.uri)} onContextMenu={(ev) => openMenu(ev, e)}>
          <span className="row-ico"><Icon name={e.is_dir ? "folder" : "note"} size={20} /></span>
          <span className="name">{e.name}</span>
          <span className="meta">{e.is_audio && e.format ? fmtFormat(e.format) : ""}{e.size ? ` · ${fmtBytes(e.size)}` : ""}</span>
          <button className="icon-btn" onClick={(ev) => { ev.stopPropagation(); openMenu(ev, e); }}><Icon name="dots" size={18} /></button>
        </div>
      ))}
      {!loading && entries.length === 0 && <div className="empty">Dossier vide.</div>}

      {menu && (
        <>
          <div className="menu-overlay" onClick={() => setMenu(null)} onContextMenu={(e) => { e.preventDefault(); setMenu(null); }} />
          <div className="ctx-menu" style={{ left: menu.x, top: menu.y }}>
            <div className="ctx-title">{menu.entry.name}</div>
            {!menu.entry.is_dir && menu.entry.is_audio && <div className="ctx-item" onClick={() => { playUri(menu.entry.uri); setMenu(null); }}><Icon name="play" size={16} filled /> Lire</div>}
            {menu.entry.is_dir && <div className="ctx-item" onClick={() => { playUri(menu.entry.uri); setMenu(null); }}><Icon name="play" size={16} filled /> Tout lire</div>}
            <div className="ctx-item" onClick={() => { act(() => api.queueAdd(menu.entry.uri)); setMenu(null); }}><Icon name="plus" size={16} /> Ajouter à la file</div>
            <div className="ctx-item" onClick={() => { window.location.href = api.exportUrl(menu.entry.uri, menu.entry.name.replace(/\.[^.]+$/, "")); setMenu(null); }}><Icon name="download" size={16} /> Télécharger (ZIP)</div>
            <div className="ctx-item" onClick={() => { setCopyTarget(menu.entry); setMenu(null); }}><Icon name="copy" size={16} /> Copier vers une autre source…</div>
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

function Playlists({ act, say, playAll, playTrack }: { act: (fn: () => Promise<unknown>, ok?: string) => Promise<void>; say: (m: string) => void; playAll: (t: Track[]) => void; playTrack: (t: Track) => void }) {
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
        {pls.map((p) => (
          <div key={p.id} className="card" onClick={() => api.playlist(p.id).then(setDetail).catch((e) => say(String(e)))}>
            <div className="cover"><Icon name="playlist" size={44} /></div>
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

      <h2>Vos sources</h2>
      {sources.length === 0 && <div className="empty">Aucune source configurée.</div>}
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

function SettingsView({ settings, updateSettings, say, refresh }: {
  settings: Settings | null;
  updateSettings: (patch: (s: Settings) => Settings, ok?: string) => Promise<void>;
  say: (m: string) => void;
  refresh: () => Promise<void>;
}) {
  const [q, setQ] = useState("");
  const [storage, setStorage] = useState<StorageInfo | null>(null);
  const [logs, setLogs] = useState<string | null>(null);
  const [busy, setBusy] = useState("");

  useEffect(() => {
    api.systemStorage().then(setStorage).catch(() => {});
  }, []);

  if (!settings) return <div className="empty">Chargement des réglages…</div>;

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
      title: "Lecture",
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
      ].filter(Boolean) as React.ReactNode[],
    },
    {
      title: "Apparence",
      rows: [
        show("Thème", "Sombre, clair, ou suivre le système") && (
          <SetRow key="theme" label="Thème" desc="Sombre, clair, ou suivre les préférences du système">
            <select value={ap.theme} onChange={(e) => setAppearance("theme", e.target.value as Settings["appearance"]["theme"])}>
              <option value="dark">Sombre</option>
              <option value="light">Clair</option>
              <option value="system">Système</option>
            </select>
          </SetRow>
        ),
        show("Couleur d'accent", "Couleur principale de l'interface (#RRGGBB)") && (
          <SetRow key="accent" label="Couleur d'accent" desc="Couleur principale de l'interface">
            <div className="range-ctrl">
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
      title: "Système",
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
      title: "Réseau",
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
      title: "Sources",
      rows: [
        show("Découverte de métadonnées musicales", "Tags et pochettes extraits à l'indexation") && (
          <SetRow key="metadata" label="Découverte de métadonnées musicales" desc="Extraction des tags et pochettes lors du scan">
            <Switch checked={so.metadata_discovery} label="métadonnées" onChange={(v) => setSources("metadata_discovery", v)} />
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
        <input type="text" placeholder="Rechercher dans les réglages…" value={q} onChange={(e) => setQ(e.target.value)} />
      </div>
      {visible.length === 0 && <div className="empty">Aucun réglage ne correspond à « {q} ».</div>}
      {visible.map((sec) => (
        <section key={sec.title} className="set-card">
          <h2>{sec.title}</h2>
          {sec.rows}
        </section>
      ))}
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

// ---------- Barre de lecture ----------

function PlayerBar({ state, devices, act, say }: { state: PlayerState | null; devices: AudioDevice[]; act: (fn: () => Promise<unknown>, ok?: string) => Promise<void>; say: (m: string) => void }) {
  const playing = state?.status === "playing";
  const [menuOpen, setMenuOpen] = useState(false);
  const cur = state?.current;
  const pos = state?.position_secs ?? 0;
  const dur = state?.duration_secs ?? cur?.duration_secs ?? 0;
  return (
    <footer className="player">
      <div className="now">
        {cur ? (<><div className="cover">{cur.has_cover ? <img src={api.coverUrl(cur.id)} alt="" /> : <Icon name="note" size={22} />}</div>
          <div className="meta"><div className="title">{cur.title}</div><div className="artist">{cur.artist}</div></div></>) : <div className="meta"><div className="title">Aucune lecture</div></div>}
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
          <span>{fmtTime(pos)}</span>
          <input type="range" min={0} max={dur || 0} step={0.1} value={Math.min(pos, dur || 0)} aria-label="Progression"
            onChange={(e) => act(() => api.seek(Number(e.target.value)))} />
          <span>{fmtTime(dur)}</span>
        </div>
      </div>
      <div className="right">
        {state?.output_mode === "browser" && <span className="badge" title="Le son sort dans ce navigateur"><Icon name="speaker" size={12} /> Navigateur</span>}
        <button className={`icon-btn-out ${state?.output_mode === "browser" ? "active" : ""}`} title="Choisir la sortie (Jouer ici, Caster…)" onClick={() => setMenuOpen((v) => !v)}>
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
        <select value={state?.audio_device ?? ""} onChange={(e) => act(() => api.selectDevice(e.target.value || null))} aria-label="Périphérique audio">
          <option value="">DAC par défaut</option>
          {devices.map((d) => <option key={d.name} value={d.name}>{d.name}{d.supports_i24 ? " (DoP)" : ""}</option>)}
        </select>
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
