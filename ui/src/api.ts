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

export interface PlaybackSettings {
  volume_curve: "logarithmic" | "linear";
  max_volume: number;
  volume_mode: "software" | "disabled";
  gapless: boolean;
  resampling: boolean;
  dsd_mode: "dop" | "pcm";
}

export interface AppearanceSettings {
  theme: "dark" | "light" | "system";
  accent: string;
  background: string | null;
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
  removeQueueItem: (index: number) => req<PlayerState>(`/api/queue/${index}`, del()),
  tracks: () => req<Track[]>("/api/library/tracks"),
  albums: () => req<Album[]>("/api/library/albums"),
  artists: () => req<Artist[]>("/api/library/artists"),
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
  browseTree: (uri: string) => req<BrowseEntry[]>(`/api/browse/tree?uri=${encodeURIComponent(uri)}`),
  usbDevices: () => req<MountInfo[]>("/api/usb/devices"),
  copy: (from: string, to: string) => req<CopyReport>("/api/transfer/copy", post({ from, to })),
  exportUrl: (uri: string, name?: string) =>
    `/api/export?uri=${encodeURIComponent(uri)}${name ? `&name=${encodeURIComponent(name)}` : ""}`,
  albumExportUrl: (albumId: string) => `/api/library/albums/${encodeURIComponent(albumId)}/export`,
  playlists: () => req<Playlist[]>("/api/playlists"),
  addPlaylist: (name: string, trackIds?: string[]) =>
    req<Playlist>("/api/playlists", post({ name, track_ids: trackIds })),
  playlist: (id: string) => req<PlaylistDetail>(`/api/playlists/${encodeURIComponent(id)}`),
  deletePlaylist: (id: string) => req<unknown>(`/api/playlists/${encodeURIComponent(id)}`, del()),
  playPlaylist: (id: string) => req<PlayerState>(`/api/playlists/${encodeURIComponent(id)}/play`, post({})),
  listShares: () => req<ShareLink[]>("/api/shares"),
  createShare: (kind: ShareLink["kind"], id: string, expires_in_secs?: number) =>
    req<ShareLink>("/api/shares", post({ kind, id, expires_in_secs })),
  deleteShare: (token: string) => req<{ ok: boolean }>(`/api/shares/${encodeURIComponent(token)}`, del()),
  shareUrl: (token: string) => `${location.origin}/share/${token}`,
  devices: () => req<AudioDevice[]>("/api/audio/devices"),
  selectDevice: (name: string | null) => req<PlayerState>("/api/audio/device", post({ name })),
  system: () => req<SystemInfo>("/api/system/info"),
  updateCheck: () => req<UpdateInfo>("/api/system/update"),
  settings: () => req<Settings>("/api/settings"),
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
