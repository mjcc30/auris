// i18n d'Auris : la clé de traduction est le texte français (source de vérité),
// `t()` retourne la traduction du pack de langue actif, ou la clé elle-même
// (fallback FR) si la clé n'est pas traduite.
//
// Packs de langue : JSON `{ "clé FR": "translation" }`, servis par le binaire
// via `/api/packs/locale/{lang}.json` — embarqués (`ui/public/locales/*.json`)
// ou communautaires (`<data-dir>/locales/{lang}.json`).

export type Dict = Record<string, string>;

let currentLocale = "fr";
let overlay: Dict = {};

export function getLocale(): string {
  return currentLocale;
}

/** Active la langue : `pack` = dictionnaire du pack (en JSON), ou undefined pour le FR natif. */
export function setLocale(locale: string, pack?: Dict): void {
  currentLocale = locale || "fr";
  overlay = pack ?? {};
}

/** Traduit `key` (texte FR) avec des paramètres `{nom}` optionnels. */
export function t(key: string, params?: Record<string, string | number>): string {
  let s = overlay[key] ?? key;
  if (params) {
    for (const [k, v] of Object.entries(params)) {
      s = s.split(`{${k}}`).join(String(v));
    }
  }
  return s;
}

/** Charge le pack de langue servi par le binaire (`/api/packs/locale/{lang}.json`). */
export async function loadLocalePack(lang: string): Promise<Dict | undefined> {
  if (lang === "fr") return undefined; // FR = clés natives
  try {
    const r = await fetch(`/api/packs/locale/${encodeURIComponent(lang)}.json`);
    if (!r.ok) return undefined;
    const j = await r.json();
    return typeof j === "object" && j !== null ? (j as Dict) : undefined;
  } catch {
    return undefined;
  }
}
