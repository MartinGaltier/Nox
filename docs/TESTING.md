# Tests, vérifications et limites connues

Mis à jour par le coordinateur à chaque intégration. Une vérification « manuelle » n'est
listée comme faite que si elle a réellement été exécutée (date et preuve).

## Automatisable (CI GitHub, `macos-15`)

`cargo fmt --all --check`, `cargo clippy --workspace --all-targets -- -D warnings`,
`cargo test --workspace` (tests marqués `#[ignore]` exclus), `scripts/build-app.sh`.

## Sur le Mac de développement (MacBook Air M2, macOS 27.0 26A428)

Voir le détail et les preuves dans `docs/parity/*.md`, `docs/METRICS_SOURCES.md`,
`docs/SLEEP_RECOVERY.md`. Synthèse en fin de projet ci-dessous.

### Vérification de bout en bout du coordinateur — 2026-10-25 01:55, `main` a731ad7

`scripts/build-app.sh` (release, arm64, ad hoc, 7,2 Mo) puis lancement de
`dist/Nox.app/Contents/MacOS/nox` avec réglages par défaut (`NOX_DATA_DIR` vide) :

| Contrôle | Résultat |
|---|---|
| Modules démarrés | music, calendar, accessories, downloads, files, shortcuts, agents, timers, companion, metrics, power, scratchpad (camera, watch, notifications, captures, clipboard désactivés par défaut) |
| Thread principal (`sample` 3 s) | 2175/2175 échantillons dans `-[NSApplication run]` : aucun gel |
| Bluetooth | `IOBluetooth` appelé uniquement sur le worker `nox-accessories` |
| Processus externes | un seul `/usr/bin/perl` (adaptateur Now Playing) |
| Mémoire | RSS 39–63 Mo |
| CPU au repos | 0,68 s sur 60 s (≈ 1,1 % d'un cœur) pendant qu'une session Claude Code active écrivait ses journaux (sondage agents 2 s) ; l'UI seule : 0,00 s/120 s (mesure Interface) |
| Assertions d'alimentation | aucune assertion Nox active au repos |
| Arrêt (SIGTERM) | app et adaptateur perl terminés en < 3 s |

## Nécessite l'utilisateur (permissions, matériel, test physique)

- Capot fermé sans écran externe (`docs/SLEEP_RECOVERY.md` § 8).
- Calendrier (autorisation Calendriers), notifications (Accessibilité), caméra, surveillance
  de zone avec l'overlay, accessoires Bluetooth réels, Musique/Spotify (aléatoire, file).
- Survol / clic / balayage physiques sur l'îlot, superposition réelle à l'encoche.

### Vérification finale — 2026-10-25 10:55, `main` fbbb0e9

Build release (`scripts/build-app.sh`, arm64, ad hoc, 7,7 Mo), réglages par défaut, données isolées :

| Contrôle | Résultat |
|---|---|
| Modules démarrés | 12 (mixeur, caméra, surveillance, notifications, captures, presse-papiers : désactivés par défaut) |
| Thread principal (`sample` 3 s) | 2113/2113 échantillons dans `-[NSApplication run]` : aucun gel |
| Mémoire | RSS 33 Mo, 20 threads |
| CPU au repos (60 s) | 1,01 s ≈ 1,7 % d'un cœur, **pendant qu'une session Claude Code active écrivait ses journaux** (le module Agents IA les relit toutes les 2 s). Interface seule : 0,00 s/120 s ; agents coupé : ≈ 1,1 % (mesures Interface). |
| Assertions d'alimentation au repos | aucune |
| Arrêt (SIGTERM) | app et adaptateur perl terminés en < 3 s |

## Synthèse finale

- Tests automatisés : 336 (CI GitHub verte sur `main`), `cargo fmt`, `clippy -D warnings`.
- Testé sur ce Mac (preuves dans `docs/parity/*.md`) : moniteur système complet (F1–F10), maintien
  éveillé modes 1 et 2, Now Playing (lecteur synthétique), paroles LRCLIB, téléchargements simulés,
  contrôles volume/micro/luminosité/clavier, agents IA Claude et Codex (agrégats recoupés au jeton
  près), capture ScreenCaptureKit, géométrie réelle de l'encoche, absence de vol de focus (par état),
  coût au repos de l'UI, absence de gel au lancement.
- **Non testé — nécessite l'utilisateur** :
  1. Capot fermé sans écran externe (installation du helper, mot de passe admin) — `docs/SLEEP_RECOVERY.md` § 8.
  2. Vrais taps audio : niveaux réels et mixeur (invite « Enregistrement audio du système ») — `docs/AUDIO.md`.
  3. Permissions : Calendrier, Accessibilité (notifications), Caméra, Bureau (captures), presse-papiers général.
  4. Musique/Spotify réels (aléatoire, répétition, file), accessoires Bluetooth connectés.
  5. Interactions physiques : survol, clic, balayage, glisser-déposer, plein écran, écran externe,
     superposition visuelle exacte sur l'encoche (captures d'écran refusées au terminal).
- Bloqué : A18 (îlot sur l'écran verrouillé : API privée SkyLight non retenue) ; resets « bancaires »
  Codex (connus du seul serveur de l'app Codex) ; Concentration/Ne pas déranger (pas d'API publique).
- Signature : ad hoc, non notarisée (aucune identité Developer ID disponible).
