# Notice et attributions

**Nox** est un logiciel libre distribué sous **GNU GPL-3.0-or-later** (voir `LICENSE`).

## Référence fonctionnelle : Vorssaint

Nox reproduit, sur un périmètre limité (Dynamic Island, moniteur système, maintien
éveillé), le comportement de **Vorssaint** — https://vorssaint.com —
code source : https://github.com/vorssaint/vorssaint-utils (GPL-3.0-or-later),
© les contributeurs de Vorssaint.

- Commit de référence épinglé : `e916961092686d32d455d12494ab10f65a43b73a` (2026-10-22).
- Nox est une réimplémentation en Rust. Lorsque de la logique, des constantes, des
  heuristiques ou des formats ont été adaptés depuis le code Swift de Vorssaint, le
  fichier Rust concerné le mentionne en en-tête (`Adapted from vorssaint-utils …`).
  Ces portions restent sous GPL-3.0-or-later, comme l'ensemble de Nox.
- Conformément à `TRADEMARKS.md` de Vorssaint : Nox **n'utilise pas** le nom, le
  logo, l'icône, l'identifiant de bundle, l'identité de signature, le flux de mise à
  jour ni l'habillage de Vorssaint, et ne se présente pas comme une version officielle
  ou approuvée de Vorssaint.

## Autres composants

Dépendances Rust : voir `Cargo.lock` (licences MIT/Apache-2.0 compatibles GPL-3.0).
Les noms Claude, Codex, OpenCode, GitHub Copilot, Spotify, Apple Music, etc. sont des
marques de leurs propriétaires respectifs ; Nox ne fait que lire des données locales
ou des API documentées pour les afficher.
