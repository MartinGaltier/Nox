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

### Fluidité de l'îlot et coût au repos — 2026-10-25, `main` 4833f3f

Mesures des agents `anim`, `render`, `idle` (build optimisé avec le pilote de survol
debug-only `NOX_UI_HOVER`, 30 cycles, `NOX_PERF=1` ; le pilote est absent du build livré,
vérifié par `strings`).

| Mesure | Avant | Après |
|---|---|---|
| Durée d'ouverture | ≈ 1,07 s | ≈ 0,3 s |
| Intervalle entre images | NSTimer 120 Hz non synchronisé, p50 8,3 ms, max 37 ms | CADisplayLink, 16,67 ms stables |
| Travail main-thread par image pendant le fondu | p50 ≈ 2,9 ms, p95 ≈ 5,2 ms, max 11,3 ms | p50 0,15 ms, p95 0,42 ms, max 0,55 ms |
| Redessins du texte pendant le fondu | 1 par image | 0 (4 dessins de contenu sur 30 cycles) |
| Première image de la page | jusqu'à 15,6 ms (pic de décodage de pochette 80 ms) | p50 0,52 ms, p95 0,68 ms |
| Centrage | ≤ 0,5 pt, variable selon l'historique | ≤ 0,25 pt par construction (fenêtre fixe) |
| Réveils d'interruption au repos | ≈ 20/s | ≈ 2,7/s |
| Réveils du main thread au repos | ≈ 1,1/s | ≈ 0,5/s |

Causes trouvées : minuteur non synchronisé, fenêtre redimensionnée pendant l'animation,
symboles SF rastérisés à chaque dessin (≈ 2,3 ms/image), opacité fractionnaire sur le texte
(≈ 10×), mise en page à la largeur animée ; modules qui se réveillaient en boucle (250 ms–2 s).

Vérification release (`main` fd3f3e0, réglages par défaut, données isolées) : thread principal
2256/2256 échantillons dans `-[NSApplication run]`, arrêt propre.

**Non vérifié avec de vrais événements** (Accessibilité non accordée au terminal) : survol,
réentrée par la zone chaude, clics dans la barre des menus à côté de l'encoche, focus clavier,
centrage à l'écran. Test manuel fourni : `scratchpad/manual/check.sh` (voir rapport).

Note d'exactitude : le message du commit `8e61ea1` (compagnon) affirme un coût « réduit à presque
rien ». C'est une surestimation : le gain n'a été mesuré que combiné avec le correctif agents
(`f7c94da`) — 459 → 350 réveils d'interruption par 120 s, 42 → 36 publications, session Claude
active pendant la mesure. L'historique n'a pas été réécrit.
