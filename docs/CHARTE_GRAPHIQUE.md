# Charte graphique — Auris

UI web sombre, inspirée des lecteurs de streaming modernes (Spotify-like),
adaptée à l'audiophile. Thème sombre par défaut ; thème clair et couleur
d'accent personnalisable depuis les Réglages (v0.5).

## Identité

- **Nom** : Auris (wordmark en minuscules, `auris`).
- **Baseline** : « L'oreille avant tout. »
- **Logo** : wordmark + pictogramme "onde/oreille" — 5 barres verticales de
  hauteurs croissantes puis décroissantes (onde sonore), couleur accent.
  SVG inline, grille 24 px, rayon 2 px.

## Palette

| Rôle | Nom | Hex | Usage |
|---|---|---|---|
| Fond principal | `bg` | `#121212` | arrière-plan global |
| Surface | `surface` | `#181818` | cartes, panneaux |
| Surface élevée | `surface-high` | `#282828` | survol, cartes actives |
| Bordure | `border` | `#2a2a2a` | séparateurs |
| Texte principal | `text` | `#FFFFFF` | titres, lecture en cours |
| Texte secondaire | `text-dim` | `#B3B3B3` | métadonnées, labels |
| Accent principal | `accent` | `#1ED760` (défaut, modifiable dans les Réglages) | boutons primaires, lecture en cours, slider |
| Accent DSD | `dsd` | `#E91429` | badge DSD, indicateur natif |
| Badge hi-res | `hires` | `#D4AF37` | badge PCM 24/96+, DSD |
| Erreur | `error` | `#E22134` | messages d'erreur |

Contrastes : `text`/`bg` = 19,4:1, `text-dim`/`bg` = 8,6:1, `accent`/`bg` = 8,9:1
(tous > 7:1, conforme WCAG AAA sur le fond).

## Thèmes (v0.5)

- **Sombre** (défaut) : variables `--bg #121212`, `--surface #181818`, sidebar noire.
- **Clair** : `[data-theme="light"]` — `--bg #f5f5f5`, `--surface #ffffff`,
  `--text #121212`, sidebar `#ececec`.
- **Système** : suit `prefers-color-scheme` du navigateur.
- L'accent est une variable CSS `--accent` (défaut `#1ED760`), choisie dans les
  Réglages au format `#RRGGBB` ; `--accent-dark` en est dérivé (assombri de 20).

## Typographie

- Pile système (pas de font embarquée, fonctionne offline) :
  `Inter, -apple-system, "Segoe UI", Roboto, "Helvetica Neue", Arial, sans-serif`.
- Titres : 600/700 ; corps : 400/500.
- Chiffres du lecteur (temps, volume) : `font-variant-numeric: tabular-nums`.
- Tailles : 12 (labels) / 14 (corps) / 16 (UI) / 20–24 (titres de page) / 32 (hero vitrine).

## Mise en page (desktop)

```
┌────────────┬──────────────────────────────────────┐
│  sidebar   │  contenu (grille de cartes)          │
│  280 px    │  grille 180 px, gap 16 px            │
│  logo      │                                      │
│  nav       ├──────────────────────────────────────┤
│  sources   │  player bar (fixe bas, 90 px)        │
└────────────┴──────────────────────────────────────┘
```

- Sidebar fixe gauche 280 px : logo, nav (Accueil, Bibliothèque, Sources, Plugins,
  Réglages), liste des sources.
- Player bar fixe en bas, 90 px : pochette 56 px, titre/artiste, contrôles
  (précédent, play/pause, suivant, aléatoire, répétition), barre de progression
  pleine largeur au-dessus, volume à droite.
- Cartes (album/artiste) : 180 px, pochette carrée, rayon **8 px**, ombre
  `0 8px 24px rgba(0,0,0,.5)`, survol : fond `surface-high` + élévation.
- Mobile (< 768 px) : sidebar → bottom nav, player bar compacte, cartes 140 px.

## Composants

- Boutons primaires : fond `accent`, texte noir, rayon 500 px (pilule), hauteur 40 px.
- Icônes : SVG inline 24 px, stroke 2 px, style arrondi (Material Symbols-like).
- Sliders (progression, volume) : piste 4 px `border`, remplissage `accent`,
  curseur 12 px blanc, apparaît au survol.
- Badges qualité : pilule 10 px uppercase, `hires` (PCM HD) ou `dsd` (DSD).
- Toasts : bas-droite, fond `surface-high`, bordure gauche 3 px (`accent`/`error`).

## Motion

- Durées 150–200 ms, easing `cubic-bezier(0.4, 0, 0.2, 1)`.
- Survol cartes : scale(1.02) ; clic : scale(0.98).
- Changement de piste : fondu 150 ms.
- `prefers-reduced-motion: reduce` → aucune animation.

## Accessibilité

- WCAG 2.1 AA : contrastes ci-dessus, focus visible (outline 2 px `accent`),
  navigation clavier complète, attributs ARIA sur le lecteur
  (`role="slider"` pour volume/progression, `aria-live="polite"` sur le titre en cours).
- Tailles de clic ≥ 44 px sur mobile.

## Fichiers

- Tokens CSS : `ui/src/styles/tokens.css` (variables `--bg`, `--accent`...).
- Logo : `ui/src/assets/logo.svg` (+ `logo-dark.svg` si besoin).
- Vitrine (site) : hérite de la palette, hero fond `#121212` avec dégradé subtil
  `radial-gradient` vert `#1ED760` à 8 % d'opacité.
