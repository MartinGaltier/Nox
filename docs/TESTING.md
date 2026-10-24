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
