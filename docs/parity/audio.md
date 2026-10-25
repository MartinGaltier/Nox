# Parité — Audio (B7, E9) — Agent Mesures/Audio

Détail : [`docs/AUDIO.md`](../AUDIO.md). Branche `agent/audio`. Machine : M2, macOS 27.0.

| ID | Fonction | Implémentation Nox | Validation exécutée | Statut |
|---|---|---|---|---|
| B7 | Visualiseur avec vrais niveaux | `nox_audio::LevelMeter` : tap privé non muet sur les processus du lecteur, agrégat privé, IOProc sans verrou, FFT 1024 points, 7 bandes, lissage. Opt-in `MusicSettings.live_levels` ; tap seulement en lecture + section visible | Tests unitaires de la chaîne d'analyse ; agrégat + IOProc sans tap (0,16 % d'un cœur) ; analyse 17 µs par mise à jour. **Aucun tap créé** : invite système, donc validation utilisateur (`NOX_ALLOW_TAP=1`). Branchement dans le module musique : agent Intégrations | implémentée (non testée avec tap) |
| E9 | Mixeur par app | `nox_audio::mixer_module()` (`ModuleId::Mixer`, désactivé par défaut) : aucun tap à 100 %, tap muet + re-rendu avec gain (0–200 %, rampe, limiteur doux) pour les apps réglées, fail-open (taps privés, silence pendant 8 s = refus) | Tests unitaires (regroupement, cibles, rendu) ; module lancé ici avec toutes les apps à 100 % : 9 apps, aucun tap, nettoyage au stop. **Réglage réel = tap = validation utilisateur**. UI : agent Interface ; enregistrement du module dans nox-app : coordinateur | implémentée (non testée avec tap) |
