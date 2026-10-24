# Tests, vérifications et limites connues

Mis à jour par le coordinateur à chaque intégration. Une vérification « manuelle » n'est
listée comme faite que si elle a réellement été exécutée (date et preuve).

## Automatisable (CI GitHub, `macos-15`)

`cargo fmt --all --check`, `cargo clippy --workspace --all-targets -- -D warnings`,
`cargo test --workspace` (tests marqués `#[ignore]` exclus), `scripts/build-app.sh`.

## Sur le Mac de développement (MacBook Air M2, macOS 27.0 26A428)

Voir le détail et les preuves dans `docs/parity/*.md`, `docs/METRICS_SOURCES.md`,
`docs/SLEEP_RECOVERY.md`. Synthèse en fin de projet ci-dessous.

## Nécessite l'utilisateur (permissions, matériel, test physique)

- Capot fermé sans écran externe (`docs/SLEEP_RECOVERY.md` § 8).
- Calendrier (autorisation Calendriers), notifications (Accessibilité), caméra, surveillance
  de zone avec l'overlay, accessoires Bluetooth réels, Musique/Spotify (aléatoire, file).
- Survol / clic / balayage physiques sur l'îlot, superposition réelle à l'encoche.
