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
  /** Crédits étendus lus au scan (#73). */
  composer?: string | null;
  producer?: string | null;
  label?: string | null;
  publisher?: string | null;
  isrc?: string | null;
  copyright?: string | null;
  /** Paroles embarquées — uniquement sur la route individuelle, absentes de la liste. */
  lyrics?: string | null;
}

export interface Album {
  id: string;
  title: string;
  artist: string;
  year: number | null;
  cover_track_id: string | null;
  track_count: number;
  duration_secs: number;
  /** Crédits de l'album agrégés depuis les pistes (#73). */
  label?: string | null;
  publisher?: string | null;
  copyright?: string | null;
}

export interface Artist {
  id: string;
  name: string;
  album_count: number;
  cover_track_id?: string | null;
}

export interface ArtistDetail {
  artist: Artist;
  albums: Album[];
  tracks: Track[];
  genres: string[];
  year_min: number | null;
  year_max: number | null;
  duration_secs: number;
  bio: string | null;
  cover_url: string | null;
}

/** Fiche MusicBrainz d'un artiste (page artiste enrichie #102). */
export interface ArtistMbInfo {
  mbid: string | null;
  name: string | null;
  sort_name: string | null;
  type: string | null;
  country: string | null;
  area: string | null;
  begin: string | null;
  end: string | null;
  disambiguation: string | null;
  tags: { name: string; count: number }[];
  genres: string[];
  rating: number | null;
  rating_count: number | null;
  urls: { type: string; url: string }[];
  wikipedia: string | null;
  url: string | null;
}

export interface ArtistDiscEntry {
  mbid: string;
  title: string | null;
  date: string | null;
  primary_type: string | null;
  secondary_types: string[];
  cover: string | null;
}

export interface SimilarArtist {
  name: string | null;
  mbid: string | null;
  score: number | null;
}

/** GET /api/library/artist_info/{id} — métadonnées riches d'un artiste (#102). */
export interface ArtistInfo {
  artist_id: string;
  name: string;
  mbid: string | null;
  mb: ArtistMbInfo | null;
  similar: SimilarArtist[];
  discography: ArtistDiscEntry[];
  bio: string | null;
  bio_image: string | null;
  images: string[];
  source: string;
}

export interface GenreFacet {
  name: string;
  track_count: number;
  album_count: number;
  cover_url: string | null;
}

export interface DecadeFacet {
  decade: number;
  track_count: number;
  album_count: number;
  cover_url: string | null;
}

export interface Facets {
  genres: GenreFacet[];
  decades: DecadeFacet[];
}

/** Stats globales de la bibliothèque (#74) —équivalent Volumio `collectionstats`. */
export interface LibraryStats {
  artists: number;
  albums: number;
  tracks: number;
  playtime_secs: number;
}

export type RipPhase = "idle" | "info" | "ripping" | "done" | "error";
export interface RipTrack {
  n: number;
  duration_secs: number;
  title: string | null;
}
export interface RipStatus {
  whipper_available: boolean;
  whipper_version: string;
  phase: RipPhase;
  device: string | null;
  progress: number;
  current_track: number;
  total_tracks: number;
  toc: RipTrack[];
  output_dir: string | null;
  log: string[];
  error: string | null;
}

/** Spotify Connect (librespot) — #24 */
export interface SpotifySettings {
  enabled: boolean;
  device_name: string;
  username: string;
  password: string;
  credentials_path: string;
  bitrate: number;
  backend: string;
  alsa_device: string;
  cache_dir: string;
}
export interface SpotStatus {
  available: boolean;
  version: string;
  running: boolean;
  pid: number | null;
  device_name: string;
  backend: string;
  bitrate: number;
  last_event: string | null;
  track_id: string | null;
  position_ms: string | null;
  log: string[];
  error: string | null;
}

/** Piste de l'historique d'écoute (#69) : Track + `last_played` (timestamp Unix). */
export type HistoryTrack = Track & { last_played: number };

export type PinKind = "album" | "playlist" | "radio";

/** Épingles (#70 — « Pin anything » façon Nova) : IDs par catégorie, ordre d'ajout. */
export interface Pins {
  albums: string[];
  playlists: string[];
  radios: string[];
}

/** Épingles résolues côté serveur (#70) : albums/playlists complets, radios en IDs/URLs. */
export interface PinsResolved {
  albums: (Album & { cover_url?: string | null })[];
  playlists: Playlist[];
  radios: string[];
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

/** Chaîne du signal en cours, façon « Signal Path » Roon (#77). */
export interface SignalPath {
  service: string;
  uri: string;
  title: string;
  live: boolean;
  stream_title: string | null;
  format_kind: string;
  sample_rate: number | null;
  bits: number | null;
  channels: number | null;
  resampling: boolean;
  dsd_mode: "dop" | "pcm";
  exclusive: boolean;
  output_backend: string;
  output_mode: "local" | "browser";
  device: string | null;
  volume_mode: "software" | "disabled";
  volume_curve: "logarithmic" | "linear";
  volume: number;
  max_volume: number;
  muted: boolean;
  loudness: boolean;
  loudness_target_lufs: number;
  loudness_gain_db: number | null;
  eq_enabled: boolean;
  eq_bands: { freq_hz: number; gain_db: number }[];
  /** Étape DSP : « CamillaDSP (<profil>) » si délégué (#82), sinon « interne ». */
  dsp: string;
  crossfade_secs: number;
  gapless: boolean;
}

export interface BrowseEntry {
  name: string;
  uri: string;
  is_dir: boolean;
  size: number | null;
  format: AudioFormat | null;
  is_audio: boolean;
}

/** Serveur UPnP/DLNA découvert par SSDP (miroir du backend Rust `UpnpDevice`). */
export interface UpnpDevice {
  friendly_name: string;
  location: string;
  base_url: string;
  content_directory_control: string;
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

export interface DirSection {
  name: string;
  stations: DirStation[];
}

/** Radio contextuelle d'un artiste (#78) : genre retenu + stations. */
export interface ArtistRadio {
  artist: string;
  tag: string;
  stations: DirStation[];
}

/** Résultat du lancement d'une radio d'artiste (#78). */
export interface ArtistRadioPlayed {
  artist: string;
  tag: string;
  count: number;
  state: PlayerState;
}

/** État de l'enregistrement radio « comme à l'ancienne » (#101). */
export interface RecordStatus {
  recording: boolean;
  url: string | null;
  started_at: number | null;
  elapsed_secs: number;
  current_title: string | null;
  bytes_written: number;
  files: string[];
  output_dir: string | null;
  error: string | null;
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

/** Émission de podcast (#81) — résultat de recherche iTunes ou abonnement. */
export interface PodcastShow {
  id: string;
  title: string;
  author: string;
  feed_url: string;
  artwork: string;
  description: string;
  subscribed: boolean;
}

/** Épisode de podcast (#81) — item RSS. */
export interface PodcastEpisode {
  id: string;
  title: string;
  url: string;
  pub_date: string;
  duration_secs: number | null;
  description: string;
  image: string;
  length_bytes: number | null;
}

/** Flux RSS complet d'une émission (#81). */
export interface PodcastFeed {
  title: string;
  author: string;
  artwork: string;
  description: string;
  feed_url: string;
  episodes: PodcastEpisode[];
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
  /** Smart playlist (#98) : règles évaluées à la volée par le serveur. */
  rules?: SmartRules | null;
  /** Nombre de pistes résolues (smart) ou stockées — renvoyé par GET /api/playlists. */
  track_count?: number;
}

/** Champ filtrable d'une smart playlist (#98). */
export type SmartField =
  | "title" | "artist" | "album" | "album_artist" | "genre" | "year"
  | "duration_secs" | "track_number" | "format" | "bit_depth" | "sample_rate"
  | "source" | "has_cover";

export type SmartOp = "eq" | "ne" | "contains" | "gt" | "lt" | "gte" | "lte";

export interface SmartRule {
  field: SmartField;
  op: SmartOp;
  value: string | number | boolean;
}

export interface SmartSort {
  field: SmartField;
  desc: boolean;
}

export interface SmartRules {
  match_all: boolean;
  rules: SmartRule[];
  sort?: SmartSort | null;
  limit?: number | null;
}

export interface PlaylistDetail {
  playlist: Playlist;
  tracks: Track[];
}

export type FavKind = "track" | "album" | "artist";

/** Favoris locaux de la bibliothèque (#68) : IDs par catégorie, ordre d'ajout. */
export interface Favorites {
  tracks: string[];
  albums: string[];
  artists: string[];
}

/** Favoris résolus en objets complets (vue Favoris). */
export interface FavoritesResolved {
  tracks: Track[];
  albums: Album[];
  artists: Artist[];
}

/** Groupe de résultats de recherche (#71) : total réel + items plafonnés. */
export interface SearchGroup<T> {
  total: number;
  items: T[];
}

/** Résultats de recherche groupés par type, façon Nova (#71). */
export interface SearchResults {
  tracks: SearchGroup<Track>;
  albums: SearchGroup<Album>;
  artists: SearchGroup<Artist>;
  playlists: SearchGroup<Playlist>;
  /** Groupe radios (#38) — optionnel pour compatibilité anciennes réponses. */
  radios?: SearchGroup<RadioEntry>;
  /** Résultats des sources externes (Qobuz #25) — optionnel pour compatibilité. */
  sources?: SearchGroup<BrowseEntry>;
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
  /** Crédits étendus de la piste (#73). */
  composer: string | null;
  producer: string | null;
  label: string | null;
  publisher: string | null;
  isrc: string | null;
  copyright: string | null;
  lyrics_embedded: boolean;
  album_info: {
    id: string;
    title: string;
    artist: string;
    year: number | null;
    track_count: number;
    duration_secs: number;
    label: string | null;
    publisher: string | null;
    copyright: string | null;
    cover_url: string | null;
  } | null;
  /** Bio Wikipedia de l'artiste (null si introuvable). */
  bio: string | null;
  /** "cache" (sled), "wikipedia" ou "none". */
  bio_source: string;
  /** Fiche MusicBrainz de la recording (#26), null si rien trouvé. */
  musicbrainz: MusicBrainzInfo | null;
}

/** Fiche MusicBrainz resumée côté backend (#26). */
export interface MusicBrainzInfo {
  mbid: string | null;
  title: string | null;
  artist: string;
  release: string;
  release_date: string;
  label: string;
  length_ms: number | null;
  tags: string[];
  isrcs: string[];
  rating: number | null;
  rating_count: number | null;
  url: string | null;
}

/** Construit un objet Source sérialisable côté backend (serde taggé). */
export function makeSource(
  id: string,
  name: string,
  kind: "local" | "usb" | "smb" | "http" | "radio" | "upnp" | "qobuz" | "tidal" | "linein" | "archive" | "deezer" | "jamendo" | "youtube",
  fields: Record<string, unknown>,
): Source {
  return { id, name, enabled: true, kind, ...fields };
}

/** Périphérique d'entrée audio (capture) — source line-in #75. */
export interface InputDevice {
  name: string;
  is_default: boolean;
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
  /** Fondu enchaîné entre pistes en secondes (0 = direct, max 12). */
  crossfade_secs: number;
  resampling: boolean;
  dsd_mode: "dop" | "pcm";
  /** Minuterie de sommeil en minutes (null = désactivée, one-shot). */
  sleep_timer: number | null;
  /** Heure de réveil "HH:MM" (heure locale, null = désactivé). */
  wake_time: string | null;
  /** Égaliseur paramétrique 10 bandes. */
  eq: EqSettings;
  /** Normalisation du volume par loudness R128 (#37). */
  loudness: boolean;
  /** Cible de loudness en LUFS (-14 streaming, -23 broadcast). */
  loudness_target_lufs: number;
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
  /** Token d'accès (#97) — masqué ("********") quand il est défini. */
  access_token: string | null;
}

export interface NetworkSettings {
  hostname: string | null;
  dlna_renderer: boolean;
}

/** Interface réseau système (#44). */
export interface NetInterface {
  name: string;
  mac: string | null;
  up: boolean;
  loopback: boolean;
  wifi: boolean;
  ips: string[];
  default_route: boolean;
  rx_bytes: number;
  tx_bytes: number;
}

/** Plugin communautaire installé (#8 — docs/PLUGIN_SYSTEM.md). */
export interface PluginInfo {
  id: string;
  name: string;
  version: string;
  category: string;
  authors: string[];
  license: string;
  homepage: string | null;
  description: string | null;
  min_auris: string | null;
  dir: string;
  bin: string | null;
  enabled: boolean;
  running: boolean;
  last_error?: string | null;
}

/** Pair Auris découvert sur le LAN (multiroom #23). */
export interface PeerInfo {
  id: string;
  name: string;
  base: string;
  version: string;
  ts_ms: number;
}

export interface MrGroup {
  id: string;
  name: string;
  members: string[];
}

/** Offset d'horloge d'un pair mesuré par Auris (#94). */
export interface PeerClockInfo {
  peer_id: string;
  offset_ms: number;
  rtt_ms: number;
  age_ms: number;
}

export interface MultiroomState {
  self_id: string;
  self_name: string;
  peers: PeerInfo[];
  groups: MrGroup[];
  clocks?: PeerClockInfo[];
}

export interface GroupActionReport {
  group: string;
  action: string;
  ok: string[];
  failed: string[];
}

export interface SourcesSettings {
  metadata_discovery: boolean;
}

export interface LastfmSettings {
  enabled: boolean;
  api_key: string;
  shared_secret: string;
  session_key: string;
  api_url: string | null;
}

export interface MusicBrainzSettings {
  enabled: boolean;
  api_url: string | null;
}

/** Mode du pont CamillaDSP (#82) : loopback (périph. de capture), pipe (FIFO), websocket (contrôle seul). */
export type CamillaMode = "loopback" | "pipe" | "websocket";

export interface CamillaSettings {
  enabled: boolean;
  mode: CamillaMode;
  /** URL du websocket CamillaDSP (défaut ws://127.0.0.1:1234). */
  ws_url: string;
  /** loopback : nom du périphérique de capture (ex. « camilladsp ») ; pipe : chemin du FIFO. */
  capture_device: string;
  /** Profil (config) à activer via SetConfigName (null = laisser le profil courant). */
  profile: string | null;
  /** Repli sur le DSP interne si CamillaDSP est injoignable. */
  fallback_internal: boolean;
}

export interface CamillaStatus {
  reachable: boolean;
  version: string | null;
  state: string | null;
  active_profile: string | null;
  profiles: string[];
  error: string | null;
}

export interface CamillaStatusResponse {
  settings: CamillaSettings;
  /** Sonde live de l'instance (null si le pont est désactivé). */
  live: CamillaStatus | null;
  /** Dernier statut appliqué par le moteur. */
  engine: CamillaStatus;
  /** true = le flux est actuellement routé vers CamillaDSP. */
  delegated: boolean;
  device: string | null;
}

export interface Settings {
  playback: PlaybackSettings;
  appearance: AppearanceSettings;
  system: SystemSettings;
  network: NetworkSettings;
  sources: SourcesSettings;
  lastfm: LastfmSettings;
  musicbrainz: MusicBrainzSettings;
  spotify: SpotifySettings;
  camilla: CamillaSettings;
}

export interface StorageInfo {
  data_dir: string;
  used_bytes: number;
  total_bytes: number;
  free_bytes: number;
}

export interface WebdavTarget {
  url: string;
  name?: string;
  user?: string;
  password?: string;
}

export interface BackupRestoreReport {
  files: number;
  restored: Record<string, number | boolean>;
  config_restored: boolean;
}

export interface BackupPushReport {
  pushed: string;
  bytes: number;
}

// ---------- Cache client des GET idempotents ----------
// La navigation entre vues démonte/remonte les composants : sans cache, chaque retour
// sur Bibliothèque/Radios re-télécharge tout (16 k pistes…). TTL court + dédup in-flight.
const getCache = new Map<string, { at: number; data: unknown }>();
const inflight = new Map<string, Promise<unknown>>();

function cached<T>(key: string, ttlMs: number, fn: () => Promise<T>): Promise<T> {
  const c = getCache.get(key);
  if (c && Date.now() - c.at < ttlMs) return Promise.resolve(c.data as T);
  const inf = inflight.get(key);
  if (inf) return inf as Promise<T>;
  const p = fn()
    .then((d) => {
      getCache.set(key, { at: Date.now(), data: d });
      return d;
    })
    .finally(() => inflight.delete(key));
  inflight.set(key, p);
  return p;
}

/** Invalide les entrées de cache dont la clé commence par l'un des préfixes donnés. */
export function bustCache(...prefixes: string[]): void {
  for (const k of [...getCache.keys()]) {
    if (prefixes.some((p) => k.startsWith(p))) getCache.delete(k);
  }
}

async function errMsg(r: Response): Promise<string> {
  let msg = `HTTP ${r.status}`;
  try {
    const j = await r.json();
    if (j.error) msg = j.error;
  } catch { /* garde le status */ }
  return msg;
}

async function req<T>(path: string, init?: RequestInit): Promise<T> {
  const r = await fetch(path, init);
  if (r.status === 401 && !path.startsWith("/api/auth/")) {
    // token d'accès requis (#97) : l'app affiche l'écran de connexion
    window.dispatchEvent(new CustomEvent("auris:unauthorized"));
  }
  if (!r.ok) {
    let msg = `HTTP ${r.status}`;
    try {
      const j = await r.json();
      if (j.error) msg = j.error;
    } catch { /* garde le status */ }
    throw new Error(msg);
  }
  if (r.status === 204) return null as T;
  const txt = await r.text();
  return (txt ? JSON.parse(txt) : null) as T;
}

export interface AuthStatus {
  required: boolean;
  authenticated: boolean;
}

export const api = {
  /** Token d'accès configuré ? (#97) — public, sert d'écran de connexion. */
  authStatus: () => req<AuthStatus>("/api/auth/status"),
  authLogin: (token: string) => req<{ ok: boolean; required: boolean }>("/api/auth/login", post({ token })),
  authLogout: () => req<{ ok: boolean }>("/api/auth/logout", post({})),
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
  tracks: (genre?: string, decade?: number) => {
    const p = new URLSearchParams();
    if (genre) p.set("genre", genre);
    if (decade != null) p.set("decade", String(decade));
    const qs = p.toString();
    const key = `tracks${qs ? `?${qs}` : ""}`;
    return cached<Track[]>(key, 15000, () => req<Track[]>(`/api/library/tracks${qs ? `?${qs}` : ""}`));
  },
  /** Page paginée de pistes (#89) : `{total, items}` — jamais mise en cache. */
  tracksPage: (o: { limit?: number; offset?: number; q?: string; genre?: string; decade?: number }) => {
    const p = new URLSearchParams();
    p.set("limit", String(o.limit ?? 200));
    p.set("offset", String(o.offset ?? 0));
    if (o.q) p.set("q", o.q);
    if (o.genre) p.set("genre", o.genre);
    if (o.decade != null) p.set("decade", String(o.decade));
    return req<{ total: number; items: Track[] }>(`/api/library/tracks/page?${p}`);
  },
  track: (id: string) => req<Track>(`/api/library/tracks/${encodeURIComponent(id)}`),
  facets: () => cached<Facets>("facets", 30000, () => req<Facets>("/api/library/facets")),
  libraryStats: () => cached<LibraryStats>("stats", 15000, () => req<LibraryStats>("/api/library/stats")),
  history: (limit = 50) => cached<HistoryTrack[]>(`history?${limit}`, 15000, () => req<HistoryTrack[]>(`/api/library/history?limit=${limit}`)),
  pins: () => cached<Pins>("pins", 15000, () => req<Pins>("/api/pins")),
  pinsResolved: () => cached<PinsResolved>("pinsResolved", 15000, () => req<PinsResolved>("/api/pins/resolved")),
  togglePin: (kind: PinKind, id: string) =>
    req<{ kind: PinKind; id: string; pinned: boolean }>("/api/pins/toggle", post({ kind, id }))
      .then((r) => { bustCache("pins"); return r; }),
  favorites: () => cached<Favorites>("favs", 15000, () => req<Favorites>("/api/favorites")),
  favoritesResolved: () => cached<FavoritesResolved>("favsResolved", 15000, () => req<FavoritesResolved>("/api/favorites/resolved")),
  toggleFavorite: (kind: FavKind, id: string) =>
    req<{ kind: FavKind; id: string; favorited: boolean }>("/api/favorites/toggle", post({ kind, id }))
      .then((r) => { bustCache("favs"); return r; }),
  albums: () => cached<Album[]>("albums", 20000, () => req<Album[]>("/api/library/albums")),
  artists: () => cached<Artist[]>("artists", 20000, () => req<Artist[]>("/api/library/artists")),
  artist: (id: string) => req<ArtistDetail>(`/api/library/artists/${encodeURIComponent(id)}`),
  search: (q: string, source?: string) =>
    req<SearchResults>(
      `/api/library/search?q=${encodeURIComponent(q)}${source ? `&source=${encodeURIComponent(source)}` : ""}`
    ),
  scan: (sourceId: string) =>
    req<unknown>("/api/library/scan", post({ source_id: sourceId }))
      .then((r) => { bustCache("tracks", "albums", "artists", "facets", "stats", "history"); return r; }),
  scanStatus: () => req<unknown>("/api/library/scan/status"),
  sources: () => cached<Source[]>("sources", 15000, () => req<Source[]>("/api/sources")),
  addSource: (s: Source) => req<Source>("/api/sources", post(s)).then((r) => { bustCache("sources"); return r; }),
  deleteSource: (id: string) => req<unknown>(`/api/sources/${encodeURIComponent(id)}`, del()).then((r) => { bustCache("sources"); return r; }),
  toggleSource: (id: string, enabled: boolean) =>
    req<unknown>(`/api/sources/${encodeURIComponent(id)}`, {
      method: "PATCH",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({ enabled }),
    }).then((r) => { bustCache("sources"); return r; }),
  updateSource: (id: string, s: Source) =>
    req<Source>(`/api/sources/${encodeURIComponent(id)}`, {
      method: "PUT",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({ ...s, id }),
    }).then((r) => { bustCache("sources"); return r; }),
  browse: (uri: string) => req<BrowseEntry[]>(`/api/browse?uri=${encodeURIComponent(uri)}`),
  radios: () => cached<RadioList>("radios", 15000, () => req<RadioList>("/api/radios")),
  multiroomState: () => req<MultiroomState>("/api/multiroom/state"),
  mrCreateGroup: (name: string) => req<MrGroup>("/api/multiroom/groups", post({ name })),
  mrDeleteGroup: (id: string) => req<{ removed: boolean }>(`/api/multiroom/groups/${encodeURIComponent(id)}`, del()),
  mrSetMember: (groupId: string, member: string, add: boolean) =>
    req<MrGroup>(`/api/multiroom/groups/${encodeURIComponent(groupId)}/members`, post({ member, add })),
  mrPlay: (group: string, uri?: string, position?: number) =>
    req<GroupActionReport>("/api/multiroom/play", post({ group, uri, position })),
  mrPause: (group: string) => req<GroupActionReport>("/api/multiroom/pause", post({ group })),
  mrStop: (group: string) => req<GroupActionReport>("/api/multiroom/stop", post({ group })),
  mrSeek: (group: string, secs: number) => req<GroupActionReport>("/api/multiroom/seek", post({ group, secs })),
  mrVolume: (group: string, volume: number, member?: string) =>
    req<GroupActionReport>("/api/multiroom/volume", post({ group, volume, member })),
  addRadio: (name: string, url: string) =>
    req<RadioList>("/api/radios", post({ name, url })).then((r) => { bustCache("radios"); return r; }),
  deleteRadio: (id: string) =>
    req<RadioList>(`/api/radios/${encodeURIComponent(id)}`, del()).then((r) => { bustCache("radios"); return r; }),
  playRadio: (id: string) => req<PlayerState>(`/api/radios/${encodeURIComponent(id)}/play`, post({})),
  playRadioUrl: (name: string, url: string) => req<PlayerState>("/api/radios/play-url", post({ name, url })),
  radiosDirGenres: () => req<DirTag[]>("/api/radios/dir/genres"),
  radiosDirCountries: () => req<DirCountry[]>("/api/radios/dir/countries"),
  radiosDirStations: (kind: string, value = "", limit = 100) =>
    req<DirStation[]>(`/api/radios/dir/stations?kind=${encodeURIComponent(kind)}&value=${encodeURIComponent(value)}&limit=${limit}`),
  radiosSelection: () => req<DirSection[]>("/api/radios/selection"),
  artistRadio: (name: string) => req<ArtistRadio>(`/api/radios/artist?name=${encodeURIComponent(name)}`),
  playArtistRadio: (name: string) => req<ArtistRadioPlayed>("/api/radios/artist-radio", post({ name })),
  // enregistrement radio (#101)
  recordStatus: () => req<RecordStatus>("/api/radio/record/status"),
  recordStart: (url: string) => req<RecordStatus>("/api/radio/record/start", post({ url })),
  recordStop: () => req<RecordStatus>("/api/radio/record/stop", post({})),
  /** Podcasts (#81) : q vide = abonnements, sinon recherche iTunes. */
  podcasts: (q = "", limit = 30) => req<PodcastShow[]>(`/api/podcasts?q=${encodeURIComponent(q)}&limit=${limit}`),
  podcastFeed: (url: string) => req<PodcastFeed>(`/api/podcasts/feed?url=${encodeURIComponent(url)}`),
  podcastSubscriptions: () => req<PodcastShow[]>("/api/podcasts/subscriptions"),
  podcastSubscribe: (s: { feed_url: string; title: string; artwork: string; author: string }) =>
    req<PodcastShow[]>("/api/podcasts/subscribe", post(s)),
  podcastUnsubscribe: (id: string) => req<PodcastShow[]>(`/api/podcasts/subscriptions/${encodeURIComponent(id)}`, del()),
  playPodcastEpisode: (e: { url: string; title: string; show: string; duration_secs?: number | null }) =>
    req<PlayerState>("/api/podcasts/play", post(e)),
  browseTree: (uri: string) => req<BrowseEntry[]>(`/api/browse/tree?uri=${encodeURIComponent(uri)}`),
  usbDevices: () => req<MountInfo[]>("/api/usb/devices"),
  /** Découverte SSDP des serveurs UPnP/DLNA du LAN (cache 30 s côté serveur). */
  upnpDiscover: (force?: boolean) => req<UpnpDevice[]>(`/api/upnp/discover${force ? "?force=true" : ""}`),
  copy: (from: string, to: string) => req<CopyReport>("/api/transfer/copy", post({ from, to })),
  exportUrl: (uri: string, name?: string) =>
    `/api/export?uri=${encodeURIComponent(uri)}${name ? `&name=${encodeURIComponent(name)}` : ""}`,
  albumExportUrl: (albumId: string) => `/api/library/albums/${encodeURIComponent(albumId)}/export`,
  playlists: () => cached<Playlist[]>("playlists", 15000, () => req<Playlist[]>("/api/playlists")),
  addPlaylist: (name: string, trackIds?: string[], uri?: string, rules?: SmartRules) =>
    req<Playlist>("/api/playlists", post({ name, track_ids: trackIds, uri, rules: rules ?? null })).then((r) => { bustCache("playlists"); return r; }),
  updatePlaylist: (id: string, patch: { name?: string; rules?: SmartRules | null }) =>
    req<Playlist>(`/api/playlists/${encodeURIComponent(id)}`, patchReq(patch)).then((r) => { bustCache("playlists"); return r; }),
  playlist: (id: string) => req<PlaylistDetail>(`/api/playlists/${encodeURIComponent(id)}`),
  deletePlaylist: (id: string) =>
    req<unknown>(`/api/playlists/${encodeURIComponent(id)}`, del()).then((r) => { bustCache("playlists"); return r; }),
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
  inputDevices: () => req<InputDevice[]>("/api/audio/input-devices"),
  dopSetup: (rate: number, device?: string | null) =>
    req<DopSetupResp>("/api/audio/dop-setup", post({ rate, device: device ?? null })),
  restartAudio: () => req<{ ok: boolean; message: string }>("/api/audio/restart-audio", post({})),
  setExclusive: (enabled: boolean) => req<{ ok: boolean; enabled: boolean }>("/api/audio/exclusive", post({ enabled })),
  system: () => req<SystemInfo>("/api/system/info"),
  updateCheck: () => req<UpdateInfo>("/api/system/update"),
  settings: () => req<Settings>("/api/settings"),
  signalPath: () => req<SignalPath>("/api/signal-path"),
  netInterfaces: () => req<NetInterface[]>("/api/network/interfaces"),
  netInterfaceAction: (name: string, action: "up" | "down") =>
    req<NetInterface[]>("/api/network/interfaces", post({ name, action })),
  plugins: () => req<PluginInfo[]>("/api/plugins"),
  pluginInstall: (path: string) => req<PluginInfo>("/api/plugins/install", post({ path })),
  pluginEnable: (id: string) => req<PluginInfo>(`/api/plugins/${encodeURIComponent(id)}/enable`, post({})),
  pluginDisable: (id: string) => req<PluginInfo>(`/api/plugins/${encodeURIComponent(id)}/disable`, post({})),
  pluginUninstall: (id: string) => req<{ uninstalled: string }>(`/api/plugins/${encodeURIComponent(id)}`, del()),
  pluginCall: (id: string, method: string, params: unknown) =>
    req<unknown>(`/api/plugins/${encodeURIComponent(id)}/call`, post({ method, params: params ?? {} })),
  pluginTest: (id: string) => req<unknown>(`/api/plugins/${encodeURIComponent(id)}/test`, post({})),
  loudnessScan: () => req<{ started: boolean; total: number; cached: number }>("/api/library/loudness/scan", post({})),
  loudnessOf: (trackId: string) =>
    req<{ track_id: string; lufs: number | null; cached_count: number }>(`/api/library/loudness/${encodeURIComponent(trackId)}`),
  packs: () => req<Packs>("/api/packs"),
  /** Contenu brut d'un pack (édition) — #96. */
  packThemeCss: (id: string) =>
    fetch(`/api/packs/theme/${encodeURIComponent(id)}`).then((r) => {
      if (!r.ok) throw new Error(`HTTP ${r.status}`);
      return r.text();
    }),
  packLocaleJson: (id: string) =>
    fetch(`/api/packs/locale/${encodeURIComponent(id)}`).then((r) => {
      if (!r.ok) throw new Error(`HTTP ${r.status}`);
      return r.text();
    }),
  saveThemePack: (id: string, css: string) =>
    req<PackEntry>(`/api/packs/theme/${encodeURIComponent(id)}`, postText(css, "text/css; charset=utf-8")),
  deleteThemePack: (id: string) => req<null>(`/api/packs/theme/${encodeURIComponent(id)}`, { ...del(), body: undefined }),
  saveLocalePack: (lang: string, json: string) =>
    req<PackEntry>(`/api/packs/locale/${encodeURIComponent(lang)}`, postText(json, "application/json; charset=utf-8")),
  deleteLocalePack: (lang: string) => req<null>(`/api/packs/locale/${encodeURIComponent(lang)}`, { ...del(), body: undefined }),
  alarms: () => req<AlarmsInfo>("/api/alarms"),
  camillaStatus: () => req<CamillaStatusResponse>("/api/camilla/status"),
  camillaProfile: (name: string) => req<CamillaStatusResponse>("/api/camilla/profile", post({ name })),
  camillaRefresh: () => req<CamillaStatusResponse>("/api/camilla/refresh", post({})),
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
  backup: async (): Promise<Blob> => {
    const r = await fetch("/api/settings/backup");
    if (!r.ok) throw new Error(await errMsg(r));
    return r.blob();
  },
  restore: async (file: Blob): Promise<BackupRestoreReport> => {
    const r = await fetch("/api/settings/restore", {
      method: "POST",
      headers: { "Content-Type": "application/zip" },
      body: file,
    });
    if (!r.ok) throw new Error(await errMsg(r));
    return r.json();
  },
  backupWebdav: (p: WebdavTarget) => req<BackupPushReport>("/api/settings/backup/webdav", post(p)),
  restoreWebdav: (p: WebdavTarget) => req<BackupRestoreReport>("/api/settings/restore/webdav", post(p)),
  lastfmTest: () => req<{ ok: boolean; name: string | null; playcount: unknown; url: string | null }>("/api/lastfm/test", post({})),
  lastfmToken: () => req<{ token: string; auth_url: string }>("/api/lastfm/token", post({})),
  lastfmSession: (token: string) => req<{ session_key: string; name: string }>("/api/lastfm/session", post({ token })),
  reboot: () => req<unknown>("/api/system/reboot", post({})),
  shutdown: () => req<unknown>("/api/system/shutdown", post({})),
  ripStatus: () => req<RipStatus>("/api/ripcd/status"),
  ripInfo: (device?: string | null) => req<RipStatus>("/api/ripcd/info", post({ device: device ?? null })),
  ripStart: (device?: string | null) => req<RipStatus>("/api/ripcd/rip", post({ device: device ?? null })),
  ripStop: () => req<RipStatus>("/api/ripcd/stop", post({})),
  ripEject: (device?: string | null) => req<{ ok: boolean; stderr: string }>("/api/ripcd/eject", post({ device: device ?? null })),
  spotifyStatus: () => req<SpotStatus>("/api/spotify/status"),
  spotifyStart: () => req<SpotStatus>("/api/spotify/start", post({})),
  spotifyStop: () => req<SpotStatus>("/api/spotify/stop", post({})),
  coverUrl: (trackId: string) => `/api/library/cover/${encodeURIComponent(trackId)}`,
  albumImageUrl: (albumId: string) => `/api/library/album_image/${encodeURIComponent(albumId)}`,
  artistImageUrl: (artistId: string) => `/api/library/artist_image/${encodeURIComponent(artistId)}`,
  artistImageByNameUrl: (name: string) => `/api/artist_image/name/${encodeURIComponent(name)}`,
  artistInfo: (artistId: string) => req<ArtistInfo>(`/api/library/artist_info/${encodeURIComponent(artistId)}`),
  lyrics: (trackId: string) => req<LyricsResponse>(`/api/lyrics/${encodeURIComponent(trackId)}`),
  trackLyrics: (trackId: string) => req<LyricsResponse>(`/api/track_lyrics/${encodeURIComponent(trackId)}`),
  infos: (trackId: string) => req<InfosResponse>(`/api/infos/${encodeURIComponent(trackId)}`),
};

function post(body: unknown): RequestInit {
  return {
    method: "POST",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify(body),
  };
}

function postText(body: string, contentType: string): RequestInit {
  return { method: "POST", headers: { "Content-Type": contentType }, body };
}

function patchReq(body: unknown): RequestInit {
  return {
    method: "PATCH",
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
