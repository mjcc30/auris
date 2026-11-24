import { useCallback, useEffect, useRef, useState } from "react";
import { api, fmtBytes, fmtTime, type Album, type AudioDevice, type BrowseEntry, type PlayerState, type Source, type SystemInfo, type Track } from "./api";

type View = "home" | "library" | "browse" | "sources";

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
    toastTimer.current = window.setTimeout(() => setToast(null), 3500);
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

  return (
    <div className="app">
      <div className="main">
        <Sidebar view={view} setView={setView} sources={sources} browseSrc={browseSrc} setBrowse={(id) => { setBrowseSrc(id); setView("browse"); }} system={system} />
        <main className="content">
          {view === "home" && <Home state={state} playTrack={playTrack} playAll={playAll} />}
          {view === "library" && <Library playTrack={playTrack} playAll={playAll} say={say} refresh={refresh} />}
          {view === "browse" && <Browse sources={sources} src={browseSrc} playUri={(u) => act(() => api.play(u))} say={say} />}
          {view === "sources" && <Sources sources={sources} refresh={refresh} say={say} />}
        </main>
      </div>
      <PlayerBar state={state} devices={devices} act={act} />
      {toast && <div className="toast">{toast}</div>}
    </div>
  );
}

function Sidebar({ view, setView, sources, browseSrc, setBrowse, system }: {
  view: View; setView: (v: View) => void; sources: Source[];
  browseSrc: string | null; setBrowse: (id: string) => void; system: SystemInfo | null;
}) {
  return (
    <nav className="sidebar">
      <div className="logo"><img src="/favicon.svg" alt="" /> Auris</div>
      <div className={`nav-item ${view === "home" ? "active" : ""}`} onClick={() => setView("home")}><span className="ico">⌂</span> Accueil</div>
      <div className={`nav-item ${view === "library" ? "active" : ""}`} onClick={() => setView("library")}><span className="ico">♫</span> Bibliothèque</div>
      <div className={`nav-item ${view === "browse" ? "active" : ""}`} onClick={() => setView("browse")}><span className="ico">🗂</span> Parcourir</div>
      <div className={`nav-item ${view === "sources" ? "active" : ""}`} onClick={() => setView("sources")}><span className="ico">⚙</span> Sources &amp; réglages</div>
      <div className="sep" />
      <div className="sources-list">
        {sources.map((s) => (
          <div key={s.id} className={`src-item ${view === "browse" && browseSrc === s.id ? "active" : ""}`} onClick={() => setBrowse(s.id)}>
            <span>{s.enabled ? <span className="dot">● </span> : "○ "}{s.name}</span><span className="badge">{s.kind}</span>
          </div>
        ))}
      </div>
      {system && <div className="badge">v{system.version} · {system.track_count} titres</div>}
    </nav>
  );
}

function Home({ state, playTrack, playAll }: { state: PlayerState | null; playTrack: (t: Track) => void; playAll: (t: Track[]) => void }) {
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
  useEffect(() => {
    api.tracks().then(setTracks).catch(() => {});
    api.albums().then(setAlbums).catch(() => {});
    api.artists().then(setArtists).catch(() => {});
  }, []);
  const doSearch = async () => {
    if (!q.trim()) { api.tracks().then(setTracks); return; }
    setTracks(await api.search(q));
  };
  const scan = async (sourceId: string) => {
    setScanning(true);
    try {
      await api.scan(sourceId);
      say(`Scan de ${sourceId} lancé`);
      setTimeout(() => { refresh(); setScanning(false); }, 1500);
    } catch (e) { say(e instanceof Error ? e.message : String(e)); setScanning(false); }
  };
  return (
    <div>
      <h1>Bibliothèque</h1>
      <div className="searchbar">
        <input type="text" placeholder="Rechercher un titre, un artiste…" value={q}
          onChange={(e) => setQ(e.target.value)} onKeyDown={(e) => e.key === "Enter" && doSearch()} style={{ flex: 1, maxWidth: 420 }} />
        <button className="btn ghost" onClick={doSearch}>Rechercher</button>
        <button className="btn" disabled={scanning} onClick={() => scan("local1")}>{scanning ? "Scan…" : "Scanner la source locale"}</button>
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
            <div key={a.id} className="card">
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

function TrackTable({ tracks, current, playTrack, playAll }: { tracks: Track[]; current?: string | null; playTrack: (t: Track) => void; playAll: (t: Track[]) => void }) {
  if (tracks.length === 0) return <div className="empty">Aucun titre.</div>;
  return (
    <table className="tracks">
      <thead><tr><th>#</th><th>Titre</th><th>Artiste</th><th>Album</th><th>Format</th><th>Durée</th></tr></thead>
      <tbody>
        {tracks.map((t, i) => (
          <tr key={t.id} className={current === t.id ? "playing" : ""} onDoubleClick={() => playTrack(t)} onClick={() => playAll(tracks.slice(i))} style={{ cursor: "pointer" }}>
            <td className="num">{i + 1}</td>
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

function Browse({ sources, src, playUri, say }: { sources: Source[]; src: string | null; playUri: (u: string) => void; say: (m: string) => void }) {
  const [path, setPath] = useState<string[]>([]);
  const [entries, setEntries] = useState<BrowseEntry[]>([]);
  const [loading, setLoading] = useState(false);
  const id = src ?? sources[0]?.id ?? null;
  useEffect(() => {
    if (!id) return;
    const uri = `src://${id}/${path.join("/")}`;
    setLoading(true);
    api.browse(uri).then(setEntries).catch((e) => say(e instanceof Error ? e.message : String(e))).finally(() => setLoading(false));
  }, [id, path, sources, say]);
  if (!id) return <div className="empty">Aucune source configurée.</div>;
  const crumb = ["/", ...path];
  return (
    <div>
      <h1>Parcourir — {sources.find((s) => s.id === id)?.name ?? id}</h1>
      <div className="searchbar">
        {crumb.map((c, i) => (
          <button key={i} className="btn ghost" onClick={() => setPath(path.slice(0, i))}>{c}</button>
        ))}
      </div>
      {loading && <div className="empty">Chargement…</div>}
      {!loading && entries.map((e) => (
        <div key={e.uri} className="browse-row" onDoubleClick={() => (e.is_dir ? setPath([...path, e.name]) : playUri(e.uri))}
          onClick={() => e.is_audio && playUri(e.uri)}>
          <span className="ico">{e.is_dir ? "📁" : "♫"}</span>
          <span className="name">{e.name}</span>
          <span className="meta">{e.is_audio && e.format ? fmtFormat(e.format) : ""}{e.size ? ` · ${fmtBytes(e.size)}` : ""}</span>
        </div>
      ))}
      {!loading && entries.length === 0 && <div className="empty">Dossier vide.</div>}
    </div>
  );
}

function Sources({ sources, refresh, say }: { sources: Source[]; refresh: () => Promise<void>; say: (m: string) => void }) {
  const [name, setName] = useState("");
  const [path, setPath] = useState("");
  const add = async () => {
    if (!name || !path) return;
    try {
      await api.addSource({ id: name.toLowerCase().replace(/\s+/g, "-"), name, enabled: true, kind: "local", path });
      say("Source ajoutée"); setName(""); setPath(""); await refresh();
    } catch (e) { say(e instanceof Error ? e.message : String(e)); }
  };
  return (
    <div>
      <h1>Sources</h1>
      {sources.map((s) => (
        <div key={s.id} className="browse-row"><span className="ico">💾</span><span className="name">{s.name}</span><span className="meta">{s.kind} · {String((s as Record<string, unknown>).path ?? "")}</span></div>
      ))}
      <h2>Ajouter une source locale</h2>
      <div className="modal" style={{ width: 480 }}>
        <label>Nom<input type="text" value={name} onChange={(e) => setName(e.target.value)} placeholder="Ma musique" /></label>
        <label>Chemin<input type="text" value={path} onChange={(e) => setPath(e.target.value)} placeholder="/mnt/nas/musique" /></label>
        <button className="btn" onClick={add}>Ajouter</button>
      </div>
    </div>
  );
}

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
