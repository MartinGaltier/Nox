# Matrice de parité — Nox vs Vorssaint `e916961`

Statuts : **à faire** · **en cours** · **implémentée** (code + vérif automatique) · **testée** (validée sur le Mac M2 réel, preuve notée) · **bloquée** (raison).
Une ligne « implémentée » n'est pas « testée » tant qu'une validation physique n'a pas été exécutée.

Détail, preuves et commandes de vérification par domaine : `docs/parity/{interface,integrations,activities,metrics,power}.md`, `docs/METRICS_SOURCES.md`, `docs/SLEEP_RECOVERY.md`. Dernière mise à jour : 2026-10-24.

Légende permissions : CAL Calendriers · CAM Caméra · AX Accessibilité · AE Automatisation (Apple Events) · FDA Accès complet au disque · SCR Enregistrement d'écran · BT Bluetooth · ADM admin une fois (helper).

## A. Îlot — cadre, interactions, intégration système

| ID | Fonction | Comportement Vorssaint | Apps/services | Permissions / dépendances | Implémentation Nox | Critères de validation | Statut |
|---|---|---|---|---|---|---|---|
| A1 | Fenêtre îlot dans l'encoche | Panneau au-dessus de la barre de menus, épouse la découpe caméra (silhouette), noir | — | aucune | `nox-ui` NSPanel non activant, niveau statusBar+1, rendu CG | Capture d'écran : îlot aligné sur l'encoche M2, pas de liseré | implémentée — géométrie vérifiée sur le M2 (encoche réelle) ; superposition visuelle non capturée |
| A2 | États compact / déplié | Compact autour de l'encoche (activité en cours), déplié au survol/clic | — | — | Machine d'états + ressort | Survol → déplie ; sortie → replie après délai (`notchCloseDelay`) | implémentée — survol physique non testé |
| A3 | Survol, délai, clic | `notchHoverExpands`, `notchHoverDelay`, `notchOpenOnHover` | — | — | Tracking area, délai réglable | Réglages appliqués sans redémarrage | implémentée — survol/clic physiques non testés |
| A4 | Animations fluides | Ressort SwiftUI, fondu contenu | — | — | Interpolation ressort Rust 60–120 Hz uniquement pendant l'animation | Aucune frame timer au repos (CPU ≈ 0) | testée (CPU UI au repos 0,00 s/120 s ; app complète ≈ 1 % d'un cœur) |
| A5 | Aucun vol de focus | Panneau non activant | — | — | `NSWindowStyleMaskNonactivatingPanel`, `becomesKeyOnlyIfNeeded` | L'app au premier plan garde le focus clavier après clic sur l'îlot | implémentée — vérifiée par état (jamais key/main, app jamais active) ; clic physique non testé |
| A6 | Écrans sans encoche | Îlot simulé (`capsule`), choix d'écran `notchDisplay` (auto/intégré/principal/pointeur/tous) | — | — | `DisplayChoice`, encoche simulée | Écran externe : capsule centrée ; changement d'écran reconfiguré | implémentée — écran externe non testé |
| A7 | Changement d'écran / résolution | Repositionnement | — | — | `NSApplicationDidChangeScreenParametersNotification` | Brancher/débrancher écran : îlot correct | implémentée — branchement d'écran non testé |
| A8 | Plein écran | `notchHideInFullscreen` masque l'îlot | — | — | Détection app plein écran sur l'écran de l'îlot | Vidéo plein écran : îlot masqué, revient après | implémentée — plein écran non testé physiquement |
| A9 | Sections activables / réorganisables | `notchModuleOrder`, `notchHiddenModules`, `notchHomeModule`, `notchReturnHome` | — | — | `IslandSettings.sections` | Désactiver une section la retire et arrête son module | implémentée |
| A10 | Gestes défilement / balayage | `notchGesturesEnabled` : balayage horizontal = section, défilement = volume/progression, `notchHapticFeedback` | — | — | `scrollWheel:` + `NSHapticFeedbackManager` | Balayage trackpad change de section | implémentée — geste trackpad physique non testé |
| A11 | Alertes / activités en direct | Notices temporaires (fin DL, agent, minuteur, batterie, accessoire…) `notchOpensActivity` | — | — | `Hub::push_alert` + file prioritaire | Alerte visible TTL puis retour | implémentée |
| A12 | Barre des menus | Icône + menu (ouvrir réglages, maintien éveillé, quitter) | — | — | `NSStatusItem` | Menu fonctionnel | implémentée |
| A13 | Réglages | Pages par fonctionnalité | — | — | Fenêtre AppKit native | Chaque réglage persiste (settings.json) | implémentée (6 onglets, aucun contrôle hors champ) |
| A14 | Arrêt des collectes à la désactivation | Fonctionnalités désinstallables | — | — | `ModuleManager` stop/join | Test auto `disabling_a_module_stops_it` + `ps -M` threads | implémentée (core, testée en automatique) |
| A15 | Masqué dans les captures | `notchHideInCaptures` / `notchShowInCaptures` | — | — | `NSWindowSharingNone` optionnel | Capture d'écran sans îlot si réglé | implémentée |
| A16 | Masquer jusqu'au survol | `notchHideUntilHover`, `notchHiddenShowsActivity` | — | — | Option UI | — | implémentée |
| A17 | Taille / silhouette / contour / translucide | `notchSize` (compact/spacieux/perso), `notchSilhouette`, `notchOutlineEnabled`, `notchTranslucentBackground`, Liquid Glass | — | — | Réglages d'apparence | — | implémentée |
| A18 | Écran verrouillé | `notchLockScreen` : îlot sur l'écran de verrouillage + sons | — | API privée SkyLight (fenêtre au-dessus du lock) | À évaluer | — | bloquée (fenêtre sur l'écran verrouillé = API privée SkyLight, non retenue) |
| A19 | Raccourci global | `notchShortcut` ouvre l'îlot | — | — | Carbon `RegisterEventHotKey` (pas d'AX) | — | implémentée |
| A20 | Contenu au repos | `notchIdleContent` (rien/batterie/musique/agents) | — | — | Réglage | — | implémentée |

## B. Musique

| ID | Fonction | Comportement Vorssaint | Apps/services | Permissions / dépendances | Implémentation Nox | Critères de validation | Statut |
|---|---|---|---|---|---|---|---|
| B1 | Lecture en cours (titre, artiste, album, pochette) | MediaRemote via adaptateur chargé dans `/usr/bin/perl` | Toute app publiant Now Playing (Musique, Spotify, navigateurs, Podcasts…) | aucune TCC ; perl système | cdylib Rust `nox-nowplaying` + script perl, flux continu | Lecture Spotify/Musique → infos et pochette exactes | testée (source synthétique ; Spotify/Musique à valider) |
| B2 | Commandes lecture/pause/suivant/précédent | MediaRemote send command | idem | — | idem | Boutons pilotent le lecteur | testée (source synthétique) |
| B3 | Progression + seek | Barre de progression, glisser pour chercher | idem | — | `elapsed_now` extrapolé | Seek déplace la lecture | testée (source synthétique) |
| B4 | Lecture aléatoire / répétition | `NotchShuffleService` | Musique, Spotify | AE | AppleScript (NSAppleScript main thread) | Bascule visible dans le lecteur | implémentée — non testée (nécessite Musique/Spotify) |
| B5 | Paroles synchronisées | `NotchLyricsService` : LRCLIB en ligne (`notchLyricsOnline`), synchro ligne | LRCLIB | réseau | Client HTTP + cache disque | Ligne active suit la lecture | implémentée ; requête LRCLIB réelle testée |
| B6 | File des prochains morceaux | `NowPlayingQueue` + AppleScript | Musique (et Spotify selon API) | AE / MediaRemote | idem | ≥ 3 prochains titres corrects | implémentée partielle (Musique estimé, Spotify non supporté) |
| B7 | Visualisation audio | `notchLiveEqualizer` (niveaux réels via `NotchAudioLevelService`) sinon barres animées | — | Enregistrement audio système (process tap) ou animation | Process tap Core Audio si dispo, sinon animation marquée « non mesurée » | Barres réagissent au son | bloquée (vrais niveaux = capture audio système ; animation non mesurée) |
| B8 | Pochettes navigateur | `notchBrowserCovers` (YouTube etc.) | Safari, Chrome | — | Artwork MediaRemote + fallback | — | implémentée (opt-in) ; requêtes iTunes et miniature YouTube réelles testées ; session Firefox sur fixtures |
| B9 | Lecteur préféré | `notchPreferredPlayer`, `notchIncludeOtherPlayers` | — | — | `MusicSettings.preferred_player` | — | implémentée ; sélection testée (2 lecteurs) ; commandes vers un lecteur non courant refusées par macOS (repli AppleScript Musique/Spotify) |
| B10 | Activité changement de piste | `notchTrackChange`, `notchMusicActivity` : compact montre pochette + égaliseur | — | — | Alerte compacte | — | implémentée |

## C. Calendrier, notifications

| ID | Fonction | Comportement Vorssaint | Apps/services | Permissions / dépendances | Implémentation Nox | Critères de validation | Statut |
|---|---|---|---|---|---|---|---|
| C1 | Événements à venir | EventKit, bande + vue mois, `notchCalendarExcluded`, numéros de semaine | Calendrier macOS (iCloud, Google, Exchange configurés) | CAL | `objc2-event-kit` | Liste = Calendrier.app sur 7 j | implémentée — non testée (permission Calendrier) |
| C2 | Compte à rebours / temps restant | `notchCalendarCountdown`, `notchCalendarTimeLeft`, `ChosenCountdowns` | — | CAL | — | — | implémentée (données) |
| C3 | Rejoindre visio | Lien Meet/Zoom/Teams détecté | — | CAL | parse URL/notes | — | implémentée |
| C4 | Alerte avant événement | Activité îlot | — | CAL | `alert_minutes_before` | — | implémentée |
| C5 | Notifications système dans l'îlot | Lit les bannières de `com.apple.notificationcenterui` via AX, option masquer les natives (`notchDismissNativeNotifications`) | Toutes apps | AX | `AXUIElement` observer | Notification Messages → affichée dans l'îlot | implémentée — non testée (Accessibilité) ; masquage natif non fait |

## D. Activités

| ID | Fonction | Comportement Vorssaint | Apps/services | Permissions / dépendances | Implémentation Nox | Critères de validation | Statut |
|---|---|---|---|---|---|---|---|
| D1 | Minuteurs | `notchTimerMinutes`, règle de temps, son (`notchTimerSoundEnabled`), compte à rebours compact | — | — | `nox-activities::timers` | Minuteur 1 min sonne et alerte | implémentée (son non écouté) |
| D2 | Chronomètre | `notchTimerMode` | — | — | idem, tours | — | implémentée |
| D3 | Concentration (Pomodoro) | focus/pause courte/longue/intervalle/sessions | — | — | idem | Transitions de phase + alertes | implémentée |
| D4 | Agents IA — Claude | Sessions live (hooks/logs `~/.claude/projects`), tokens, modèles, projets, limites 5 h/semaine + reset (API OAuth usage), valeur API estimée | Claude Code CLI, app Claude | lecture fichiers ; jeton OAuth Keychain si opt-in | Lecteur JSONL incrémental | Session réelle détectée, tokens = somme logs | testée (journaux locaux, agrégats) ; OAuth opt-in non exécutée |
| D5 | Agents IA — Codex | `~/.codex/sessions` rollouts, `rate_limits` dans les logs, comptes `CODEX_HOME` multiples, resets bancaires | Codex CLI | lecture fichiers | idem | — | testée (jetons, limites) ; resets bancaires bloqués |
| D6 | Agents IA — OpenCode | `~/.local/share/opencode` storage | OpenCode | lecture fichiers | idem | — | implémentée (OpenCode absent : fixtures) |
| D7 | Agents IA — GitHub Copilot | Logs session CLI/app, métriques à l'arrêt | Copilot CLI | lecture fichiers | idem | — | implémentée (Copilot absent : fixtures) |
| D8 | Alertes fin de tâche / limite | `notchAgentsFinishAlert`, `FinishMinimum`, `LimitAlert`, `LimitThreshold` | — | — | Alertes hub | Tâche > 30 s finie → alerte | implémentée |
| D9 | Estimations coût / budget | `AgentPricing`, `notchAgentsDailyBudget`, `PriceUpdates` | Table de prix | réseau optionnel | `Reading::Estimated` | Affiché « estimé » | implémentée (estimé) |
| D10 | Surveillance de zone | `NotchWatchService` : choisir zone, alerte changement / fin (`notchWatchCondition`), son | — | SCR | ScreenCaptureKit / CGWindowList | Zone qui change → alerte | implémentée (capture réelle OK ; parcours UI à valider) |
| D11 | Compagnon animé | Mascotte : forme, palette, côté, style, visites, réactions aux événements | — | — | Rendu CG animé, humeur dérivée de l'état | Réagit (musique, agent, batterie) | implémentée (rendu : nox-ui) |

## E. Accessoires, téléchargements, fichiers, caméra, raccourcis, contrôles

| ID | Fonction | Comportement Vorssaint | Apps/services | Permissions / dépendances | Implémentation Nox | Critères de validation | Statut |
|---|---|---|---|---|---|---|---|
| E1 | Accessoires connectés + batterie | `NotchAccessoryService` : AirPods (G/D/boîtier), claviers/souris, alerte connexion | Bluetooth, IOKit HID | BT (selon méthode) | IORegistry `BatteryPercent` + défauts Bluetooth | AirPods connectés → % affichés | implémentée ; IORegistry testé, gel Bluetooth corrigé et vérifié ; aucun accessoire connecté |
| E2 | Alertes batterie interne | `notchLowBatteryThreshold`, `Early`, teinte, menu barre | — | — | Depuis `nox-metrics` | Seuil franchi → alerte | implémentée (tests unitaires) |
| E3 | Suivi des téléchargements | Dossier Téléchargements (signet), `.crdownload/.download/.part`, progression | Safari, Chrome, Firefox, Arc… | accès dossier | FSEvents + taille fichiers | Téléchargement réel → progression puis fin | testée (téléchargement simulé) |
| E4 | Outils de fichiers (étagère) | Déposer des fichiers sur l'îlot, AirDrop, partager, compresser, `notchDragReveal` | Finder | — | Drag & drop NSPanel | Glisser un fichier → apparaît, AirDrop ouvre | implémentée ; compression testée |
| E5 | Aperçu caméra | Miroir dans l'îlot | — | CAM | AVCaptureSession + preview layer, démarré à l'ouverture seulement | Voyant caméra s'allume seulement page ouverte | implémentée — non testée (permission + UI) |
| E6 | Raccourcis / actions rapides | Accès rapide (verrouiller, capture, etc.), `notchQuickAccess*` | Raccourcis.app | — | `nox-integrations::shortcuts` | Chaque action exécutée | implémentée partielle (Concentration/NPD sans API publique) |
| E7 | Contrôles volume / luminosité / clavier / micro / source | `notchVolume`, `Brightness`, `KeyboardLight`, `Microphone`, `InputSource`, ordre `notchControlOrder` | — | — | CoreAudio, DisplayServices (privé) | Curseurs agissent | implémentée ; volume/micro/luminosité testés, rétroéclairage clavier vérifié (sélecteurs macOS 27), sources d'entrée implémentées |
| E8 | HUD volume/luminosité dans l'îlot | `NotchVolumeKeyGate` | — | AX (event tap) | À évaluer | — | implémentée (HUD dans l'îlot via écouteurs CoreAudio/DisplayServices, HUD système conservé ; notification luminosité testée) |
| E9 | Mixeur par app | section `mixer` | — | process taps CoreAudio | hors priorité (utilitaire séparé) | — | à faire (évalué : capture audio) |
| E10 | Presse-papiers dans l'îlot | section `clipboard` | — | — | Historique local | — | implémentée (opt-in, mémoire seule) ; pasteboard privé testé ; alerte macOS sur le presse-papiers général = validation utilisateur |
| E11 | Captures récentes | section `captures` | — | — | Dossier captures | — | implémentée (fixtures + rendu) ; vraie capture non testée, module désactivé par défaut |
| E12 | Bloc-notes | section `scratchpad` | — | — | Fichier local | — | implémentée (fichier temporaire testé, éditeur natif) |

## F. Moniteur système (îlot + fenêtre)

| ID | Mesure | Source Vorssaint | Source Nox (voir docs/METRICS_SOURCES.md) | Validation | Statut |
|---|---|---|---|---|---|
| F1 | CPU global + par cœur (P/E) | `host_processor_info` | idem (Mach) | Comparer Moniteur d'activité | testée |
| F2 | GPU | IOAccelerator `PerformanceStatistics` | idem (IOKit) | Comparer `ioreg` | testée |
| F3 | Températures CPU/GPU/batterie/SSD | SMC `SMCClient` + HID | SMC user client / IOHIDEventSystem | Plausibles, cohérentes sous charge | testée |
| F4 | RAM, pression, swap | `host_statistics64`, `vm.swapusage`, `kern.memorystatus_vm_pressure_level` | idem | Comparer `vm_stat`, `sysctl` | testée |
| F5 | Batterie : charge, secteur, santé, cycles, autonomie | IOPS + AppleSmartBattery | idem | Comparer `ioreg`/`pmset -g batt` | testée (santé estimée, état indisponible) |
| F6 | Puissance (système, CPU, GPU) | SMC `PSTR` / IOReport | SMC + IOReport si fiable | Ordre de grandeur cohérent | testée (système/GPU ; CPU/ANE indisponibles) |
| F7 | Débit réseau | `getifaddrs` | idem | Comparer `nettop`/`netstat -ib` | testée |
| F8 | Espace disque | `statfs` / volumes | idem | Comparer `df -h` | testée (`/` seulement) |
| F9 | Historique borné | Graphes | `MetricsHistory` (trous = None) | Test auto | testée |
| F10 | Ventilateurs | SMC | SMC (M2 Air : sans ventilateur → Indisponible) | — | testée (aucun ventilateur → Indisponible) |

## G. Maintien éveillé

| ID | Fonction | Méthode Vorssaint | Méthode Nox | Validation | Statut |
|---|---|---|---|---|---|
| G1 | Mac actif, écran peut s'éteindre | `IOPMAssertion PreventUserIdleSystemSleep` | idem | `pmset -g assertions` montre l'assertion Nox | testée |
| G2 | Mac + écran actifs | + `PreventUserIdleDisplaySleep` | idem | idem | testée |
| G3 | Capot fermé sans écran externe | `pmset disablesleep 1` via règle sudoers NOPASSWD installée avec mot de passe admin | Helper launchd root minimal (socket Unix, liste blanche d'opérations), restauration au crash/reboot par le helper lui-même ; **pas de sudoers** | Test physique capot fermé (non exécutable par l'agent) | implémentée — NON testée physiquement |
| G4 | Durées 15/30/60/120 min/illimitée | Oui | `KeepAwakeDuration::PRESETS` | Expiration exacte | implémentée |
| G5 | Indicateur mode + temps restant | Îlot + barre de menus | idem | — | implémentée (données ; UI en cours) |
| G6 | Arrêt manuel îlot + menu | Oui | idem | — | implémentée (données ; UI en cours) |
| G7 | Uniquement sur secteur | Oui | `only_on_ac` | Débrancher → arrêt | implémentée |
| G8 | Arrêt seuil batterie | Oui | `stop_below_percent` | — | implémentée |
| G9 | Restauration état précédent | Mémorise `SleepDisabled` initial | Journal helper + vérif avant restauration (ne pas écraser un changement utilisateur) | Tests unitaires de la logique | implémentée |
