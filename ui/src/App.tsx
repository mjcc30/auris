import { useCallback, useEffect, useRef, useState } from "react";

import { api, fmtBytes, fmtTime, makeSource, type Album, type AudioDevice, type BrowseEntry, type CopyReport, type MountInfo, type PlaylistDetail, type PlayerState, type Source, type SystemInfo, type Track } from "./api";

type View = "home" | "library" | "browse" | "queue" | "playlists" | "sources";

function kindIcon(kind: string): string {
  switch (kind) {
    case "usb": return "🔌";
    case "smb": return "🖴";
    case "upnp": return "📡";
    case "radio": return "📻";
    case "http": return "🔗";
    default: return "📁";
  }
}

function kindLabel(kind: string): string {
  switch (kind) {
    case "usb": return "USB";
    case "smb": return "SMB (NAS)";
    case "upnp": return "UPnP/DLNA";
    case "radio": return "Radio";
    case "http": return "HTTP";
    default: return "Dossier";
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

export default function App() {
  const [view, setView] = useState<View>("home");
  const [state, setState] = useState<PlayerState | null>(null);
  const [sources, setSources] = useState<Source[]>([]);
  const [devices, setDevices] = useState<AudioDevice[]>([]);
  const [system, setSystem] = useState<SystemInfo | null>(null);
  const [browseSrc, setBrowseSrc] = useState<string | null>(null);
  const [toast, setToast] = useState<string | null>(null);
  const toastTimer = useRef<number | null>(null);

  const say = useCallback((msg: string) => {
    setToast(msg);
    if (toastTimer.current) window.clearTimeout(toastTimer.current);
    toastTimer.current = window.setTimeout(() => setToast(null), 4500);
  }, []);

  const refresh = useCallback(async () => {
    try {
      const [s, srcs, devs, sys] = await Promise.all([api.state(), api.sources(), api.devices(), api.system()]);
      setState(s); setSources(srcs); setDevices(devs); setSystem(sys);
    } catch (e) { /* backend indisponible pour l'instant */ }
  }, []);

  useEffect(() => {
    refresh();
    const ws = new WebSocket(`${location.protocol === "https:" ? "wss" : "ws"}://${location.host}/ws`);
    ws.onmessage = () => refresh();
    const iv = setInterval(refresh, 2000); // position en lecture
    return () => { ws.close(); clearInterval(iv); };
  }, [refresh]);

  const act = useCallback(async (fn: () => Promise<unknown>, okMsg?: string) => {
    try { await fn(); if (okMsg) say(okMsg); await refresh(); }
    catch (e) { say(e instanceof Error ? e.message : String(e)); }
  }, [refresh, say]);

  const playTrack = (t: Track) => act(() => api.playTrack(t.id));
  const playAll = (tracks: Track[]) => act(() => api.playQueue(tracks, 0));
  const playUri = (u: string) => act(() => api.play(u));
  const openSource = (id: string) => { setBrowseSrc(id); setView("browse"); };

  return (
    <div className="app">
      <div className="main">
        <Sidebar view={view} setView={setView} sources={sources} browseSrc={browseSrc} openSource={openSource} system={system} />
        <main className="content">
          {view === "home" && <Home state={state} sources={sources} playTrack={playTrack} playAll={playAll} openSource={openSource} />}
          {view === "library" && <Library playTrack={playTrack} playAll={playAll} say={say} refresh={refresh} />}
          {view === "browse" && <Browse sources={sources} src={browseSrc} playUri={playUri} act={act} say={say} />}
          {view === "queue" && <Queue state={state} act={act} say={say} playAll={playAll} />}
          {view === "playlists" && <Playlists act={act} say={say} playAll={playAll} playTrack={playTrack} />}
          {view === "sources" && <Sources sources={sources} refresh={refresh} say={say} />}
        </main>
      </div>
      <PlayerBar state={state} devices={devices} act={act} />
      {toast && <div className="toast">{toast}</div>}
    </div>
  );
}

function Sidebar({ view, setView, sources, browseSrc, openSource, system }: {
  view: View; setView: (v: View) => void; sources: Source[];
  browseSrc: string | null; openSource: (id: string) => void; system: SystemInfo | null;
}) {
  return (
    <nav className="sidebar">
      <div className="logo"><img src="/favicon.svg" alt="" /> Auris</div>
      <div className={`nav-item ${view === "home" ? "active" : ""}`} onClick={() => setView("home")}><span className="ico">⌂</span> Accueil</div>
      <div className={`nav-item ${view === "library" ? "active" : ""}`} onClick={() => setView("library")}><span className="ico">♫</span> Bibliothèque</div>
      <div className={`nav-item ${view === "browse" ? "active" : ""}`} onClick={() => setView("browse")}><span className="ico">🗂</span> Parcourir</div>
      <div className={`nav-item ${view === "queue" ? "active" : ""}`} onClick={() => setView("queue")}><span className="ico">≡</span> File d'attente</div>
      <div className={`nav-item ${view === "playlists" ? "active" : ""}`} onClick={() => setView("playlists")}><span className="ico">📑</span> Listes de lecture</div>
      <div className={`nav-item ${view === "sources" ? "active" : ""}`} onClick={() => setView("sources")}><span className="ico">⚙</span> Sources &amp; réglages</div>
      <div className="sep" />
      <div className="sources-list">
        {sources.map((s) => (
          <div key={s.id} className={`src-item ${view === "browse" && browseSrc === s.id ? "active" : ""}`} onClick={() => openSource(s.id)}>
            <span>{s.enabled ? <span className="dot">● </span> : "○ "}{kindIcon(s.kind)} {s.name}</span><span className="badge">{s.kind}</span>
          </div>
        ))}
      </div>
      {system && <div className="badge">v{system.version} · {system.track_count} titres</div>}
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
      <h1>Bonjour</h1>
      {state?.current && (
        <div>
          <h2>En lecture</h2>
          <div className="grid">
            <div className="card" onClick={() => playTrack(state.current!)}>
              <div className="cover">{state.current.has_cover ? <img src={api.coverUrl(state.current.id)} alt="" /> : "♫"}</div>
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
            <div className="cover">{kindIcon(s.kind)}</div>
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
            <div className="cover">{a.cover_track_id ? <img src={api.coverUrl(a.cover_track_id)} alt="" /> : "💿"}</div>
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

function Library({ playTrack, playAll, say, refresh }: { playTrack: (t: Track) => void; playAll: (t: Track[]) => void; say: (m: string) => void; refresh: () => Promise<void> }) {
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
  const doSearch = async () => {
    if (!q.trim()) { api.tracks().then(setTracks); return; }
    setTracks(await api.search(q));
  };
  const scan = async (sourceId?: string) => {
    setScanning(true);
    try {
      const id = sourceId ?? (await api.sources()).find((s) => s.enabled)?.id;
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
      <h1>Bibliothèque</h1>
      <div className="searchbar">
        <input type="text" placeholder="Rechercher un titre, un artiste…" value={q}
          onChange={(e) => setQ(e.target.value)} onKeyDown={(e) => e.key === "Enter" && doSearch()} style={{ flex: 1, maxWidth: 420 }} />
        <button className="btn ghost" onClick={doSearch}>Rechercher</button>
        <button className="btn" disabled={scanning} onClick={() => scan()}>{scanning ? "Scan…" : "Scanner la source"}</button>
      </div>
      <div className="searchbar">
        {(["tracks", "albums", "artists"] as const).map((t) => (
          <button key={t} className={`btn ghost ${tab === t ? "active" : ""}`} style={tab === t ? { borderColor: "var(--green)", color: "var(--green)" } : {}} onClick={() => setTab(t)}>
            {t === "tracks" ? "Titres" : t === "albums" ? "Albums" : "Artistes"}
          </button>
        ))}
      </div>
      {tab === "tracks" && <TrackTable tracks={tracks} playTrack={playTrack} playAll={playAll} />}
      {tab === "albums" && (
        <div className="grid">
          {albums.map((a) => (
            <div key={a.id} className="card" onClick={() => setAlbumDetail({ id: a.id, title: a.title, artist: a.artist })}>
              <div className="cover">{a.cover_track_id ? <img src={api.coverUrl(a.cover_track_id)} alt="" /> : "💿"}</div>
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
      <div className="searchbar">
        <button className="btn ghost" onClick={onBack}>← Retour</button>
        <button className="btn" onClick={() => tracks.length && playAll(tracks)}>▶ Tout lire</button>
        <button className="btn ghost" onClick={() => { window.location.href = api.albumExportUrl(id); }}>⬇ Export ZIP</button>
      </div>
      <h1>{title}</h1>
      <h2 style={{ marginTop: 0 }}>{artist}</h2>
      <TrackTable tracks={tracks} playTrack={playTrack} playAll={playAll} albumCover={cover} />
    </div>
  );
}

function TrackTable({ tracks, current, playTrack, playAll, albumCover }: { tracks: Track[]; current?: string | null; playTrack: (t: Track) => void; playAll: (t: Track[]) => void; albumCover?: string | null }) {
  if (tracks.length === 0) return <div className="empty">Aucun titre.</div>;
  return (
    <table className="tracks">
      <thead><tr><th>#</th><th>Titre</th><th>Artiste</th><th>Album</th><th>Format</th><th>Durée</th></tr></thead>
      <tbody>
        {tracks.map((t, i) => (
          <tr key={t.id} className={current === t.id ? "playing" : ""} onDoubleClick={() => playTrack(t)} onClick={() => playAll(tracks.slice(i))} style={{ cursor: "pointer" }}>
            <td className="num">{albumCover && i === 0 ? <img src={albumCover} alt="" style={{ width: 28, height: 28, borderRadius: 4, verticalAlign: "middle" }} /> : i + 1}</td>
            <td>{t.title}</td><td>{t.artist}</td><td>{t.album}</td>
            <td><span className="badge">{t.format ? fmtFormat(t.format) : ""}</span></td>
            <td>{t.duration_secs != null ? fmtTime(t.duration_secs) : "—"}</td>
          </tr>
        ))}
      </tbody>
    </table>
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

// ---------- Parcourir (arborescence des sources) ----------

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
  }, [id, path, sources, say]);

  if (!id) return <div className="empty">Aucune source configurée — ajoutez-en une dans « Sources &amp; réglages ».</div>;
  const source = sources.find((s) => s.id === id);
  const curUri = `src://${id}/${path.join("/")}`;
  const crumb = ["🏠", ...path];

  const playAllInDir = () => act(() => api.play(curUri), "Lecture du dossier…");
  const enqueueDir = () => act(() => api.queueAdd(curUri), "Ajouté à la file d'attente");

  const openMenu = (e: React.MouseEvent, entry: BrowseEntry) => {
    e.preventDefault();
    e.stopPropagation();
    setMenu({ x: e.clientX, y: e.clientY, entry });
  };

  return (
    <div>
      <div className="toolbar">
        <div className="crumb">
          {crumb.map((c, i) => (
            <span key={i}>
              {i > 0 && <span className="crumb-sep">/</span>}
              <button className="btn ghost sm" onClick={() => setPath(path.slice(0, i))}>{c}</button>
            </span>
          ))}
        </div>
        <div className="spacer" />
        <button className="btn ghost sm" onClick={playAllInDir} title="Lire tout le dossier">▶ Tout lire</button>
        <button className="btn ghost sm" onClick={enqueueDir} title="Ajouter tout le dossier à la file">+ File</button>
        <button className={`btn ghost sm ${grid ? "active" : ""}`} onClick={() => setGrid(true)} title="Grille">⊞</button>
        <button className={`btn ghost sm ${!grid ? "active" : ""}`} onClick={() => setGrid(false)} title="Liste">☰</button>
      </div>
      <h1>{kindIcon(source?.kind ?? "local")} {source?.name ?? id}{path.length > 0 && <span className="crumb-sep"> — {path.join(" / ")}</span>}</h1>
      {loading && <div className="empty">Chargement…</div>}
      {!loading && grid && (
        <div className="grid">
          {entries.map((e) => (
            <div key={e.uri} className="card" onDoubleClick={() => (e.is_dir ? setPath([...path, e.name]) : playUri(e.uri))}
              onClick={() => { if (e.is_audio) playUri(e.uri); }}
              onContextMenu={(ev) => openMenu(ev, e)} title={e.name}>
              <div className="cover">{e.is_dir ? "📁" : "♫"}</div>
              <div className="t">{e.name}</div>
              <div className="s">{e.is_dir ? "dossier" : `${e.is_audio ? "audio" : "fichier"}${e.size ? ` · ${fmtBytes(e.size)}` : ""}`}</div>
            </div>
          ))}
        </div>
      )}
      {!loading && !grid && entries.map((e) => (
        <div key={e.uri} className="browse-row" onDoubleClick={() => (e.is_dir ? setPath([...path, e.name]) : playUri(e.uri))}
          onClick={() => e.is_audio && playUri(e.uri)} onContextMenu={(ev) => openMenu(ev, e)}>
          <span className="ico">{e.is_dir ? "📁" : "♫"}</span>
          <span className="name">{e.name}</span>
          <span className="meta">{e.is_audio && e.format ? fmtFormat(e.format) : ""}{e.size ? ` · ${fmtBytes(e.size)}` : ""}</span>
          <button className="btn ghost sm" onClick={(ev) => { ev.stopPropagation(); openMenu(ev, e); }}>⋯</button>
        </div>
      ))}
      {!loading && entries.length === 0 && <div className="empty">Dossier vide.</div>}

      {menu && (
        <>
          <div className="menu-overlay" onClick={() => setMenu(null)} onContextMenu={(e) => { e.preventDefault(); setMenu(null); }} />
          <div className="ctx-menu" style={{ left: menu.x, top: menu.y }}>
            <div className="ctx-title">{menu.entry.name}</div>
            {!menu.entry.is_dir && menu.entry.is_audio && <div className="ctx-item" onClick={() => { playUri(menu.entry.uri); setMenu(null); }}>▶ Lire</div>}
            {menu.entry.is_dir && <div className="ctx-item" onClick={() => { playUri(menu.entry.uri); setMenu(null); }}>▶ Tout lire</div>}
            <div className="ctx-item" onClick={() => { act(() => api.queueAdd(menu.entry.uri)); setMenu(null); }}>+ Ajouter à la file</div>
            <div className="ctx-item" onClick={() => { window.location.href = api.exportUrl(menu.entry.uri, menu.entry.name.replace(/\.[^.]+$/, "")); setMenu(null); }}>⬇ Télécharger (ZIP)</div>
            <div className="ctx-item" onClick={() => { setCopyTarget(menu.entry); setMenu(null); }}>⇄ Copier vers une autre source…</div>
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
      else say(`Copié: ${r.files} fichier(s), ${fmtBytes(r.bytes)}`);
      onClose();
    } catch (e) { say(e instanceof Error ? e.message : String(e)); }
    finally { setBusy(false); }
  };
  return (
    <div className="modal-bg" onClick={onClose}>
      <div className="modal" style={{ width: 520 }} onClick={(e) => e.stopPropagation()}>
        <h2 style={{ margin: 0 }}>Copier « {entry.name} » vers…</h2>
        <label>Source destination
          <select value={dest} onChange={(e) => setDest(e.target.value)}>
            {writable.length === 0 && <option value="">aucune source inscriptible</option>}
            {writable.map((s) => <option key={s.id} value={s.id}>{kindIcon(s.kind)} {s.name} ({s.kind})</option>)}
          </select>
        </label>
        <label>Dossier destination (relatif, vide = racine)
          <input type="text" value={destPath} onChange={(e) => setDestPath(e.target.value)} placeholder="ex: import/2026" />
        </label>
        <div style={{ display: "flex", gap: 10, justifyContent: "flex-end" }}>
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
    const name = window.prompt("Nom de la liste de lecture:", "Ma liste");
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
        <h1 style={{ margin: 0 }}>File d'attente</h1>
        <div className="spacer" />
        <button className="btn ghost sm" disabled={saving || queue.length === 0} onClick={save}>💾 Enregistrer</button>
        <button className="btn ghost sm" disabled={queue.length === 0} onClick={() => act(() => api.clearQueue(), "File vidée")}>🗑 Vider</button>
      </div>
      {queue.length === 0 && <div className="empty">La file d'attente est vide.</div>}
      {queue.length > 0 && (
        <table className="tracks">
          <thead><tr><th>#</th><th>Titre</th><th>Artiste</th><th>Album</th><th>Durée</th><th></th></tr></thead>
          <tbody>
            {queue.map((t, i) => (
              <tr key={`${t.id}-${i}`} className={idx === i ? "playing" : ""} style={{ cursor: "pointer" }}
                onClick={() => playAll(queue.slice(i))}>
                <td className="num">{idx === i ? "▶" : i + 1}</td>
                <td>{t.title}</td><td>{t.artist}</td><td>{t.album}</td>
                <td>{t.duration_secs != null ? fmtTime(t.duration_secs) : "—"}</td>
                <td><button className="btn ghost sm" title="Retirer" onClick={(e) => { e.stopPropagation(); act(() => api.removeQueueItem(i)); }}>✕</button></td>
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
          <button className="btn ghost sm" onClick={() => { setDetail(null); load(); }}>← Retour</button>
          <h1 style={{ margin: 0 }}>{detail.playlist.name}</h1>
          <div className="spacer" />
          <button className="btn sm" onClick={() => act(() => api.playPlaylist(detail.playlist.id))}>▶ Lire</button>
          <button className="btn ghost sm" onClick={() => act(async () => { await api.deletePlaylist(detail.playlist.id); setDetail(null); load(); }, "Liste supprimée")}>🗑 Supprimer</button>
        </div>
        <TrackTable tracks={detail.tracks} playTrack={playTrack} playAll={playAll} />
      </div>
    );
  }
  return (
    <div>
      <h1>Listes de lecture</h1>
      {pls.length === 0 && <div className="empty">Aucune liste — enregistrez la file d'attente pour en créer une.</div>}
      <div className="grid">
        {pls.map((p) => (
          <div key={p.id} className="card" onClick={() => api.playlist(p.id).then(setDetail).catch((e) => say(String(e)))}>
            <div className="cover">📑</div>
            <div className="t">{p.name}</div>
            <div className="s">{p.track_ids.length} titre(s)</div>
          </div>
        ))}
      </div>
    </div>
  );
}

// ---------- Sources & réglages ----------

function Sources({ sources, refresh, say }: { sources: Source[]; refresh: () => Promise<void>; say: (m: string) => void }) {
  const [tab, setTab] = useState<"local" | "usb" | "smb" | "radio" | "http">("local");
  const [name, setName] = useState("");
  const [path, setPath] = useState("");
  const [host, setHost] = useState("");
  const [share, setShare] = useState("");
  const [user, setUser] = useState("");
  const [password, setPassword] = useState("");
  const [basePath, setBasePath] = useState("");
  const [url, setUrl] = useState("");
  const [mounts, setMounts] = useState<MountInfo[]>([]);
  const [detecting, setDetecting] = useState(false);

  const uniqueId = (base: string) => {
    let id = base;
    let n = 2;
    while (sources.some((s) => s.id === id)) { id = `${base}-${n}`; n += 1; }
    return id;
  };

  const add = async (src: Source) => {
    try {
      await api.addSource(src);
      say(`Source « ${src.name} » ajoutée`);
      setName(""); setPath(""); setHost(""); setShare(""); setUser(""); setPassword(""); setBasePath(""); setUrl("");
      await refresh();
    } catch (e) { say(e instanceof Error ? e.message : String(e)); }
  };

  const addLocal = () => name && path && add(makeSource(uniqueId(slugify(name)), name, "local", { path }));
  const addSmb = () => name && host && share && add(makeSource(uniqueId(slugify(name)), name, "smb", { host, share, user, password, base_path: basePath }));
  const addRadio = () => name && url && add(makeSource(uniqueId(slugify(name)), name, "radio", { url }));
  const addHttp = () => name && add(makeSource(uniqueId(slugify(name)), name, "http", { base_url: url || null }));
  const addUsb = (m: MountInfo) => add(makeSource(uniqueId(`usb-${slugify(m.label)}`), m.label, "usb", { path: m.path }));
  const detect = async () => {
    setDetecting(true);
    try { setMounts(await api.usbDevices()); }
    catch (e) { say(e instanceof Error ? e.message : String(e)); }
    finally { setDetecting(false); }
  };

  const srcSummary = (s: Source): string => {
    const r = s as Record<string, unknown>;
    switch (s.kind) {
      case "local": case "usb": return String(r.path ?? "");
      case "smb": return `${r.host}/${r.share}${r.base_path ? `/${r.base_path}` : ""}`;
      case "radio": case "http": return String(r.url ?? r.base_url ?? "");
      default: return "";
    }
  };

  return (
    <div>
      <h1>Sources &amp; réglages</h1>
      {sources.map((s) => (
        <div key={s.id} className="browse-row">
          <span className="ico">{kindIcon(s.kind)}</span>
          <span className="name">{s.name} <span className="badge">{kindLabel(s.kind)}</span></span>
          <span className="meta">{srcSummary(s)}</span>
          <button className={`btn ghost sm ${s.enabled ? "active" : ""}`} title={s.enabled ? "Désactiver" : "Activer"}
            onClick={async () => { try { await api.toggleSource(s.id, !s.enabled); await refresh(); } catch (e) { say(String(e)); } }}>
            {s.enabled ? "● active" : "○ off"}
          </button>
          <button className="btn ghost sm" title="Supprimer" onClick={async () => { try { await api.deleteSource(s.id); say(`Source « ${s.name} » supprimée`); await refresh(); } catch (e) { say(String(e)); } }}>✕</button>
        </div>
      ))}
      {sources.length === 0 && <div className="empty">Aucune source configurée.</div>}

      <h2>Ajouter une source</h2>
      <div className="searchbar">
        {([["local", "Dossier"], ["usb", "USB"], ["smb", "SMB (NAS)"], ["radio", "Radio"], ["http", "URL HTTP"]] as const).map(([k, label]) => (
          <button key={k} className={`btn ghost ${tab === k ? "active" : ""}`} style={tab === k ? { borderColor: "var(--green)", color: "var(--green)" } : {}} onClick={() => setTab(k)}>{label}</button>
        ))}
      </div>

      {tab === "local" && (
        <div className="modal" style={{ width: 520 }}>
          <label>Nom<input type="text" value={name} onChange={(e) => setName(e.target.value)} placeholder="Ma musique" /></label>
          <label>Chemin du dossier (NTFS monté, partage monté…)<input type="text" value={path} onChange={(e) => setPath(e.target.value)} placeholder="C:\Musique ou /mnt/nas/musique" /></label>
          <button className="btn" onClick={addLocal}>Ajouter le dossier</button>
        </div>
      )}

      {tab === "usb" && (
        <div className="modal" style={{ width: 520 }}>
          <p style={{ color: "var(--text-sub)", margin: 0 }}>Disques amovibles détectés par l'OS (branchez la clé USB puis cliquez sur Détecter).</p>
          <button className="btn ghost" disabled={detecting} onClick={detect}>{detecting ? "Détection…" : "🔌 Détecter les disques USB"}</button>
          {mounts.length === 0 && !detecting && <div className="empty">Aucun disque détecté.</div>}
          {mounts.map((m) => (
            <div key={m.path} className="browse-row">
              <span className="ico">🔌</span>
              <span className="name">{m.label}</span>
              <span className="meta">{m.path}</span>
              <button className="btn sm" onClick={() => addUsb(m)}>Ajouter</button>
            </div>
          ))}
          <label>Ou chemin manuel<input type="text" value={path} onChange={(e) => setPath(e.target.value)} placeholder="E:\ ou /media/moi/CLE" /></label>
          <label>Nom<input type="text" value={name} onChange={(e) => setName(e.target.value)} placeholder="Clé USB" /></label>
          <button className="btn" onClick={() => name && path && add(makeSource(uniqueId(`usb-${slugify(name)}`), name, "usb", { path }))}>Ajouter la clé</button>
        </div>
      )}

      {tab === "smb" && (
        <div className="modal" style={{ width: 520 }}>
          <label>Nom<input type="text" value={name} onChange={(e) => setName(e.target.value)} placeholder="NAS" /></label>
          <label>Hôte (IP ou nom)<input type="text" value={host} onChange={(e) => setHost(e.target.value)} placeholder="192.168.1.10" /></label>
          <label>Partage (share)<input type="text" value={share} onChange={(e) => setShare(e.target.value)} placeholder="music" /></label>
          <label>Utilisateur<input type="text" value={user} onChange={(e) => setUser(e.target.value)} placeholder="user" /></label>
          <label>Mot de passe<input type="password" value={password} onChange={(e) => setPassword(e.target.value)} /></label>
          <label>Sous-dossier racine (optionnel)<input type="text" value={basePath} onChange={(e) => setBasePath(e.target.value)} placeholder="flac" /></label>
          <button className="btn" onClick={addSmb}>Ajouter le partage SMB</button>
        </div>
      )}

      {tab === "radio" && (
        <div className="modal" style={{ width: 520 }}>
          <label>Nom<input type="text" value={name} onChange={(e) => setName(e.target.value)} placeholder="FIP" /></label>
          <label>URL du flux<input type="text" value={url} onChange={(e) => setUrl(e.target.value)} placeholder="https://…/stream.aac" /></label>
          <button className="btn" onClick={addRadio}>Ajouter la radio</button>
        </div>
      )}

      {tab === "http" && (
        <div className="modal" style={{ width: 520 }}>
          <label>Nom<input type="text" value={name} onChange={(e) => setName(e.target.value)} placeholder="Playlist distante" /></label>
          <label>URL (m3u/m3u8 ou fichier direct, optionnel)<input type="text" value={url} onChange={(e) => setUrl(e.target.value)} placeholder="https://…/playlist.m3u" /></label>
          <button className="btn" onClick={addHttp}>Ajouter l'URL</button>
        </div>
      )}
    </div>
  );
}

// ---------- Barre de lecture ----------

function PlayerBar({ state, devices, act }: { state: PlayerState | null; devices: AudioDevice[]; act: (fn: () => Promise<unknown>, ok?: string) => Promise<void> }) {
  const playing = state?.status === "playing";
  const cur = state?.current;
  const pos = state?.position_secs ?? 0;
  const dur = state?.duration_secs ?? cur?.duration_secs ?? 0;
  return (
    <footer className="player">
      <div className="now">
        {cur ? (<><div className="cover">{cur.has_cover ? <img src={api.coverUrl(cur.id)} alt="" /> : "♫"}</div>
          <div className="meta"><div className="title">{cur.title}</div><div className="artist">{cur.artist}</div></div></>) : <div className="meta"><div className="title">Aucune lecture</div></div>}
      </div>
      <div className="center">
        <div className="controls">
          <button className={state?.shuffle ? "active" : ""} title="Aléatoire" onClick={() => act(() => api.shuffle(!state?.shuffle))}>⇄</button>
          <button title="Précédent" onClick={() => act(() => api.prev())}>⏮</button>
          <button className="play" title={playing ? "Pause" : "Lecture"} onClick={() => act(() => (playing ? api.pause() : api.resume()))}>{playing ? "⏸" : "▶"}</button>
          <button title="Stop" onClick={() => act(() => api.stop())}>⏹</button>
          <button title="Suivant" onClick={() => act(() => api.next())}>⏭</button>
          <button className={state?.repeat !== "off" ? "active" : ""} title="Répéter" onClick={() => act(() => api.repeat(state?.repeat === "off" ? "all" : state?.repeat === "all" ? "one" : "off"))}>🔁</button>
        </div>
        <div className="seek">
          <span>{fmtTime(pos)}</span>
          <input type="range" min={0} max={dur || 0} step={0.1} value={Math.min(pos, dur || 0)}
            onChange={(e) => act(() => api.seek(Number(e.target.value)))} />
          <span>{fmtTime(dur)}</span>
        </div>
      </div>
      <div className="right">
        {state?.current_format && <span className="badge">{fmtFormat(state.current_format)}</span>}
        <select value={state?.audio_device ?? ""} onChange={(e) => act(() => api.selectDevice(e.target.value || null))}>
          <option value="">DAC par défaut</option>
          {devices.map((d) => <option key={d.name} value={d.name}>{d.name}{d.supports_i24 ? " (DoP)" : ""}</option>)}
        </select>
        <span>🔊</span>
        <input type="range" min={0} max={100} value={state?.muted ? 0 : state?.volume ?? 100}
          onChange={(e) => act(() => api.volume(Number(e.target.value)))} />
        <span style={{ width: 32, textAlign: "right" }}>{state?.muted ? 0 : state?.volume ?? 100}</span>
      </div>
    </footer>
  );
}
