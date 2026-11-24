# ADR-0003 — DSD délivré au DAC via DoP (pas de DSD natif en v1)

- Statut : accepté
- Date : 2026-11-24

## Contexte

Le DSD natif (DSD over PCM natif, marqueurs 0x05/0xFA) exige un pilote spécifique :
ASIO sur Windows, mode `DSD` ALSA sur Linux. Sur macOS/CoreAudio, seul le DoP
est possible. Un binaire cross-plateforme doit fonctionner partout.

## Décision

Les fichiers DSF/DFF sont décodés nativement (parseur maison), puis encapsulés
en **DoP 24 bits** (flux I24 cpal à dsd_rate/16) : DSD64→176,4 kHz, DSD128→352,8 kHz,
DSD256→705,6 kHz. Le DAC compatible DoP reçoit du DSD natif.
Le DSD natif OS (ASIO/ALSA direct) est reporté en v1.1 (ADR à venir).

## Conséquences

- + fonctionne sur macOS/Windows/Linux avec le même code
- + jusqu'au DSD256 supporté (DoP 24 bits)
- − nécessite un DAC compatible DoP (la majorité des DAC audiophiles l'est)
- − le volume logiciel ne s'applique pas au marqueur DoP (déjà géré : gain sur
  les 16 bits de données uniquement)
