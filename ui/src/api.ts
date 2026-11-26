// Client API Auris + types miroirs du backend Rust.

export type AudioFormat = { kind: string; sample_rate?: number; bits?: number; channels?: number };

export interface Track {
  id: string;
  title: string;
  artist: string;
  album: string;
  album_artist: string;
  duration_secs: number | null;
  track_number: number | null;
  disc_number: number | null;
  year: number | null;
  genre: string | null;
  uri: string;
  source_id: string;
  format: AudioFormat | null;
  size_bytes: number | null;
  has_cover: boolean;
}

export interface Album {
  id: string;
  title: string;
  artist: string;
  year: number | null;
  cover_track_id: string | null;
  track_count: number;
  duration_secs: number;
}

export interface Artist {
  id: string;
  name: string;
  album_count: number;
}

export interface ArtistDetail {
  artist: Artist;
  albums: Album[];
  tracks: Track[];
  cover_url: string | null;
}

export interface PlayerState {
  status: "stopped" | "playing" | "paused";
  current: Track | null;
  position_secs: number;
  duration_secs: number;
  volume: number;
  muted: boolean;
  repeat: "off" | "all" | "one";
  shuffle: boolean;
  queue: Track[];
  queue_index: number | null;
  audio_device: string | null;
  current_format: AudioFormat | null;
  max_volume: number;
  volume_mode: string;
  output_mode: "local" | "browser";
  /** Backend de sortie actif, ex. « cpal » ou « exclusif 176400 Hz, 24 bits PCM, 2 ch ». */
  output_backend: string;
  /** Flux live (webradio) : durée inconnue, lecture infinie, seek inopérant. */
  is_live: boolean;
  /** Morceau en cours sur un flux live (métadonnées ICY StreamTitle). */
  stream_title: string | null;
}

export interface Source {
  id: string;
  name: string;
  enabled: boolean;
  kind: string;
  [k: string]: unknown;
}

export interface BrowseEntry {
  name: string;
  uri: string;
  is_dir: boolean;
  size: number | null;
  format: AudioFormat | null;
  is_audio: boolean;
}

export interface MountInfo {
  label: string;
  path: string;
}

export interface RadioEntry {
  id: string;
  name: string;
  url: string;
  origin: "builtin" | "favorite" | "source";
}

export interface RadioList {
  favorites: RadioEntry[];
  builtin: RadioEntry[];
  sources: RadioEntry[];
}

export interface DirTag {
  name: string;
  count: number;
}

export interface DirCountry {
  name: string;
  code: string;
  count: number;
}

export interface DirStation {
  id: string;
  name: string;
  url: string;
  favicon: string;
  bitrate: number;
  codec: string;
  tags: string;
  country: string;
  votes: number;
}

export interface CopyReport {
  files: number;
  bytes: number;
  dirs: number;
  errors: string[];
}

export interface Playlist {
  id: string;
  name: string;
  track_ids: string[];
  created_at: number;
}

export interface PlaylistDetail {
  playlist: Playlist;
  tracks: Track[];
}

/** Lien de partage public (v0.6) : /share/<token>. */
export interface ShareLink {
  token: string;
  kind: "track" | "album" | "playlist";
  id: string;
  title: string;
  created_at: number;
  expires_at: number | null;
  url: string;
}

/** Paroles d'une piste : tag embarqué ou lrclib.net. */
export interface LyricsResponse {
  track_id: string;
  /** "tag" (USLT/LYRICS embarqué), "lrclib" (lrclib.net) ou "none". */
  source: "tag" | "lrclib" | "none" | string;
  /** Paroles en texte brut (null si aucune). */
  plain: string | null;
  /** Paroles synchronisées LRC: [[secondes, ligne], ...]. */
  synced: [number, string][];
}

/** Infos de la lecture en cours (onglet Infos du panneau). */
export interface InfosResponse {
  track_id: string;
  title: string;
  artist: string;
  album: string;
  album_artist: string;
  track_number: number | null;
  disc_number: number | null;
  year: number | null;
  genre: string | null;
  duration_secs: number | null;
  format: AudioFormat | null;
  /** Commentaire embarqué (tag COMM/COMMENT). */
  comment: string | null;
  album_info: {
    id: string;
    title: string;
    artist: string;
    year: number | null;
    track_count: number;
    duration_secs: number;
    cover_url: string | null;
  } | null;
  /** Bio Wikipedia de l'artiste (null si introuvable). */
  bio: string | null;
  /** "cache" (sled), "wikipedia" ou "none". */
  bio_source: string;
}

/** Construit un objet Source sérialisable côté backend (serde taggé). */
export function makeSource(
  id: string,
  name: string,
  kind: "local" | "usb" | "smb" | "http" | "radio" | "upnp",
  fields: Record<string, unknown>,
): Source {
  return { id, name, enabled: true, kind, ...fields };
}

export interface AudioDevice {
  name: string;
  is_default: boolean;
  sample_rates: number[];
  supports_i24: boolean;
  supports_f32: boolean;
  max_channels: number;
}

/** Mix format WASAPI d'un endpoint (backend Rust `MixFormat`). */
export interface MixFormat {
  rate: number;
  channels: number;
  bits_per_sample: number;
  is_float: boolean;
}

export interface DopDeviceStatus {
  name: string;
  /** Mix format live de l'endpoint (ce que le moteur délivre maintenant). */
  mix: MixFormat | null;
  /** Default Format persisté : adopté au prochain rebranchement du DAC / redémarrage Audio. */
  default_format: MixFormat | null;
}

export interface DopStatus {
  windows: boolean;
  current_device: string | null;
  devices: DopDeviceStatus[];
  /** Le périphérique courant accepte-t-il au moins un format en WASAPI exclusif. */
  exclusive_supported: boolean;
  /** Mode exclusif demandé (config) — appliqué au prochain flux. */
  exclusive_enabled: boolean;
}

export interface DopSetupResp {
  ok: boolean;
  applied: boolean;
  rate: number;
  device: string | null;
  mix: MixFormat | null;
  message: string;
}

/** Formate un MixFormat pour l'affichage (ex: « 176400 Hz, 24 bits PCM, 2 ch »). */
export function fmtMix(m: MixFormat | null | undefined): string {
  if (!m) return "inconnu";
  return `${m.rate} Hz, ${m.bits_per_sample} bits ${m.is_float ? "float" : "PCM"}, ${m.channels} ch`;
}

/** Porteuse DoP d'un format DSD (DSD64 -> 176400, DSD128 -> 352800…). */
export function dopCarrier(fmt: AudioFormat | null | undefined): number | null {
  if (!fmt || fmt.kind !== "dsd" || !fmt.sample_rate) return null;
  return Math.round(fmt.sample_rate / 16);
}

export interface SystemInfo {
  name: string;
  version: string;
  os: string;
  arch: string;
  uptime_secs: number;
  track_count: number;
  source_count: number;
  audio_device: string | null;
}

export interface UpdateInfo {
  current_version: string;
  latest_version: string;
  update_available: boolean;
  release_url: string | null;
  checked_at: number;
}

// ---------- Réglages (équivalent menu « Réglages » de Volumio Nova) ----------

export interface EqBand {
  freq_hz: number;
  gain_db: number;
}

export interface EqSettings {
  enabled: boolean;
  bands: EqBand[];
}

export interface PlaybackSettings {
  volume_curve: "logarithmic" | "linear";
  max_volume: number;
  volume_mode: "software" | "disabled";
  gapless: boolean;
  resampling: boolean;
  dsd_mode: "dop" | "pcm";
  /** Minuterie de sommeil en minutes (null = désactivée, one-shot). */
  sleep_timer: number | null;
  /** Heure de réveil "HH:MM" (heure locale, null = désactivé). */
  wake_time: string | null;
  /** Égaliseur paramétrique 10 bandes. */
  eq: EqSettings;
}

export interface AlarmsInfo {
  sleep_timer: number | null;
  sleep_remaining_secs: number | null;
  wake_time: string | null;
}

export interface AppearanceSettings {
  /** id du thème : "dark" | "light" | "system" ou id d'un pack CSS (ex. "aurora"). */
  theme: string;
  /** code ISO de la langue (ex. "fr", "en", "pt-BR"). */
  language: string;
  accent: string;
  background: string | null;
}

export interface PackEntry {
  id: string;
  name: string;
  builtin: boolean;
}

export interface Packs {
  themes: PackEntry[];
  locales: PackEntry[];
}

export interface SystemSettings {
  device_name: string;
  timezone: string | null;
}

export interface NetworkSettings {
  hostname: string | null;
}

export interface SourcesSettings {
  metadata_discovery: boolean;
}

export interface Settings {
  playback: PlaybackSettings;
  appearance: AppearanceSettings;
  system: SystemSettings;
  network: NetworkSettings;
  sources: SourcesSettings;
}

export interface StorageInfo {
  data_dir: string;
  used_bytes: number;
  total_bytes: number;
  free_bytes: number;
}

async function req<T>(path: string, init?: RequestInit): Promise<T> {
  const r = await fetch(path, init);
  if (!r.ok) {
    let msg = `HTTP ${r.status}`;
    try {
      const j = await r.json();
      if (j.error) msg = j.error;
    } catch { /* garde le status */ }
    throw new Error(msg);
  }
  return r.json() as Promise<T>;
}

export const api = {
  state: () => req<PlayerState>("/api/state"),
  play: (uri: string) => req<PlayerState>("/api/player/play", post({ uri })),
  playTrack: (trackId: string) => req<PlayerState>("/api/player/play", post({ track_id: trackId })),
  playQueue: (tracks: Track[], index = 0) =>
    req<PlayerState>("/api/player/play", post({ queue: tracks, index })),
  pause: () => req<PlayerState>("/api/player/pause", post({})),
  resume: () => req<PlayerState>("/api/player/resume", post({})),
  stop: () => req<PlayerState>("/api/player/stop", post({})),
  next: () => req<PlayerState>("/api/player/next", post({})),
  prev: () => req<PlayerState>("/api/player/prev", post({})),
  seek: (secs: number) => req<PlayerState>("/api/player/seek", post({ secs })),
  volume: (volume: number) => req<PlayerState>("/api/player/volume", post({ volume })),
  mute: (muted: boolean) => req<PlayerState>("/api/player/mute", post({ muted })),
  repeat: (mode: PlayerState["repeat"]) => req<PlayerState>("/api/player/repeat", post({ mode })),
  setOutput: (mode: PlayerState["output_mode"]) => req<PlayerState>("/api/player/output", post({ mode })),
  shuffle: (enabled: boolean) => req<PlayerState>("/api/player/shuffle", post({ enabled })),
  queue: () => req<Track[]>("/api/queue"),
  clearQueue: () => req<unknown>("/api/queue/clear", post({})),
  queueAdd: (uri: string) => req<PlayerState>("/api/queue/add", post({ uri })),
  queueAddNext: (uri: string) => req<PlayerState>("/api/queue/add-next", post({ uri })),
  removeQueueItem: (index: number) => req<PlayerState>(`/api/queue/${index}`, del()),
  tracks: () => req<Track[]>("/api/library/tracks"),
  albums: () => req<Album[]>("/api/library/albums"),
  artists: () => req<Artist[]>("/api/library/artists"),
  artist: (id: string) => req<ArtistDetail>(`/api/library/artists/${encodeURIComponent(id)}`),
  search: (q: string) => req<Track[]>(`/api/library/search?q=${encodeURIComponent(q)}`),
  scan: (sourceId: string) => req<unknown>("/api/library/scan", post({ source_id: sourceId })),
  scanStatus: () => req<unknown>("/api/library/scan/status"),
  sources: () => req<Source[]>("/api/sources"),
  addSource: (s: Source) => req<Source>("/api/sources", post(s)),
  deleteSource: (id: string) => req<unknown>(`/api/sources/${encodeURIComponent(id)}`, del()),
  toggleSource: (id: string, enabled: boolean) =>
    req<unknown>(`/api/sources/${encodeURIComponent(id)}`, {
      method: "PATCH",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({ enabled }),
    }),
  updateSource: (id: string, s: Source) =>
    req<Source>(`/api/sources/${encodeURIComponent(id)}`, {
      method: "PUT",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({ ...s, id }),
    }),
  browse: (uri: string) => req<BrowseEntry[]>(`/api/browse?uri=${encodeURIComponent(uri)}`),
  radios: () => req<RadioList>("/api/radios"),
  addRadio: (name: string, url: string) => req<RadioList>("/api/radios", post({ name, url })),
  deleteRadio: (id: string) => req<RadioList>(`/api/radios/${encodeURIComponent(id)}`, del()),
  playRadio: (id: string) => req<PlayerState>(`/api/radios/${encodeURIComponent(id)}/play`, post({})),
  playRadioUrl: (name: string, url: string) => req<PlayerState>("/api/radios/play-url", post({ name, url })),
  radiosDirGenres: () => req<DirTag[]>("/api/radios/dir/genres"),
  radiosDirCountries: () => req<DirCountry[]>("/api/radios/dir/countries"),
  radiosDirStations: (kind: string, value = "", limit = 100) =>
    req<DirStation[]>(`/api/radios/dir/stations?kind=${encodeURIComponent(kind)}&value=${encodeURIComponent(value)}&limit=${limit}`),
  browseTree: (uri: string) => req<BrowseEntry[]>(`/api/browse/tree?uri=${encodeURIComponent(uri)}`),
  usbDevices: () => req<MountInfo[]>("/api/usb/devices"),
  copy: (from: string, to: string) => req<CopyReport>("/api/transfer/copy", post({ from, to })),
  exportUrl: (uri: string, name?: string) =>
    `/api/export?uri=${encodeURIComponent(uri)}${name ? `&name=${encodeURIComponent(name)}` : ""}`,
  albumExportUrl: (albumId: string) => `/api/library/albums/${encodeURIComponent(albumId)}/export`,
  playlists: () => req<Playlist[]>("/api/playlists"),
  addPlaylist: (name: string, trackIds?: string[], uri?: string) =>
    req<Playlist>("/api/playlists", post({ name, track_ids: trackIds, uri })),
  playlist: (id: string) => req<PlaylistDetail>(`/api/playlists/${encodeURIComponent(id)}`),
  deletePlaylist: (id: string) => req<unknown>(`/api/playlists/${encodeURIComponent(id)}`, del()),
  playPlaylist: (id: string) => req<PlayerState>(`/api/playlists/${encodeURIComponent(id)}/play`, post({})),
  playlistAdd: (id: string, uri: string) =>
    req<{ playlist: Playlist; added: number }>(`/api/playlists/${encodeURIComponent(id)}/add`, post({ uri })),
  listShares: () => req<ShareLink[]>("/api/shares"),
  createShare: (kind: ShareLink["kind"], id: string, expires_in_secs?: number) =>
    req<ShareLink>("/api/shares", post({ kind, id, expires_in_secs })),
  deleteShare: (token: string) => req<{ ok: boolean }>(`/api/shares/${encodeURIComponent(token)}`, del()),
  shareUrl: (token: string) => `${location.origin}/share/${token}`,
  devices: () => req<AudioDevice[]>("/api/audio/devices"),
  selectDevice: (name: string | null) => req<PlayerState>("/api/audio/device", post({ name })),
  dopStatus: () => req<DopStatus>("/api/audio/dop-status"),
  dopSetup: (rate: number, device?: string | null) =>
    req<DopSetupResp>("/api/audio/dop-setup", post({ rate, device: device ?? null })),
  restartAudio: () => req<{ ok: boolean; message: string }>("/api/audio/restart-audio", post({})),
  setExclusive: (enabled: boolean) => req<{ ok: boolean; enabled: boolean }>("/api/audio/exclusive", post({ enabled })),
  system: () => req<SystemInfo>("/api/system/info"),
  updateCheck: () => req<UpdateInfo>("/api/system/update"),
  settings: () => req<Settings>("/api/settings"),
  packs: () => req<Packs>("/api/packs"),
  alarms: () => req<AlarmsInfo>("/api/alarms"),
  updateSettings: (s: Settings) =>
    req<Settings>("/api/settings", {
      method: "PUT",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify(s),
    }),
  systemStorage: () => req<StorageInfo>("/api/system/storage"),
  systemLogs: async () => {
    const r = await fetch("/api/system/logs");
    if (!r.ok) {
      let msg = `HTTP ${r.status}`;
      try { const j = await r.json(); if (j.error) msg = j.error; } catch { /* garde le status */ }
      throw new Error(msg);
    }
    return r.text();
  },
  systemReset: () => req<unknown>("/api/system/reset", post({ confirm: "RESET" })),
  reboot: () => req<unknown>("/api/system/reboot", post({})),
  shutdown: () => req<unknown>("/api/system/shutdown", post({})),
  coverUrl: (trackId: string) => `/api/library/cover/${encodeURIComponent(trackId)}`,
  lyrics: (trackId: string) => req<LyricsResponse>(`/api/lyrics/${encodeURIComponent(trackId)}`),
  infos: (trackId: string) => req<InfosResponse>(`/api/infos/${encodeURIComponent(trackId)}`),
};

function post(body: unknown): RequestInit {
  return {
    method: "POST",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify(body),
  };
}

function del(): RequestInit {
  return { method: "DELETE" };
}

export function fmtTime(secs: number): string {
  if (!isFinite(secs) || secs < 0) secs = 0;
  const m = Math.floor(secs / 60);
  const s = Math.floor(secs % 60);
  return `${m}:${s.toString().padStart(2, "0")}`;
}

export function fmtBytes(n: number | null | undefined): string {
  if (n == null) return "";
  if (n < 1024) return `${n} o`;
  if (n < 1024 * 1024) return `${(n / 1024).toFixed(1)} Ko`;
  if (n < 1024 * 1024 * 1024) return `${(n / 1024 / 1024).toFixed(1)} Mo`;
  return `${(n / 1024 / 1024 / 1024).toFixed(2)} Go`;
}
