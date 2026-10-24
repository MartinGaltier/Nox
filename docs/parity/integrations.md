# Parité — Intégrations (Agent Intégrations, branche `agent/integrations`)

Machine de test : MacBook Air M2, macOS 27.0 (26A428). Statuts selon `docs/PARITY.md` :
« testée » = exécutée sur cette machine, avec la preuve indiquée ; « implémentée » = code et tests
automatiques, mais pas de validation réelle ; « non testé : permission » = le terminal qui exécute
les tests n'a pas l'autorisation, et je ne l'ai pas demandée.

Tests réels : `cargo test -p nox-integrations -- --ignored <nom>` (liste en fin de document).
Les données personnelles (titres, événements, notifications, noms d'appareils) ne sont jamais
affichées par ces tests : seulement des compteurs.

## B. Musique

| ID | Statut | Implémentation / preuve |
|---|---|---|
| B1 | **testée** (source synthétique) | `crates/nox-nowplaying` : cdylib chargée dans `/usr/bin/perl` par `packaging/resources/now-playing.pl`, **un seul** processus long par démarrage du module, notifications MediaRemote, sans scrutation. Redémarrages bornés (3, backoff exponentiel). Preuve : un lecteur de test jetable (AVAudioPlayer muet + MPNowPlayingInfoCenter) donne le titre, l'artiste, l'album, la durée et une pochette JPEG ; `live_adapter_starts_answers_and_stops` (`NOX_EXPECT_PLAYING=1`) affiche un `MusicState` correct, puis `stop()` ne laisse aucun processus. Spotify/Musique : non testé (nécessite l'utilisateur). |
| B2 | **testée** (source synthétique) | play/pause/suivant/précédent via `MRMediaRemoteSendCommand`, chaque commande confirmée par la lecture suivante. |
| B3 | **testée** (source synthétique) | seek (option `kMRMediaRemoteOptionPlaybackPosition`, à défaut `MRMediaRemoteSetElapsedTime`) : 30,0 s relus ; position = échantillon + horodatage, extrapolée par `NowPlaying::elapsed_now`. `can_seek` vaut true seulement si la durée et la position sont connues. |
| B4 | implémentée — non testé : nécessite l'utilisateur | NSAppleScript (main thread, borné à 3 s par Apple Event, jamais lancé si l'app ne tourne pas) pour Musique (`shuffle enabled`, `song repeat`) et Spotify (`shuffling`, `repeating`). La permission Automatisation est lue sans invite ; l'invite n'apparaît qu'à une action de l'utilisateur (SetShuffle/SetRepeat/`RequestAutomation`). Spotify : la répétition d'un seul titre n'est pas scriptable. |
| B5 | implémentée ; requête LRCLIB réelle **testée** | `https://lrclib.net/api/get` via NSURLSession éphémère, cache disque dans `cache_dir()/lyrics/<hash>.json` (aucun titre en clair, mais les paroles identifient la piste) : plafonné à 200 entrées (les plus anciennes supprimées), **effacé quand les paroles sont désactivées** ; réponses négatives retentées après 7 jours. Parseur LRC (`[offset]`, balises multiples, durée) et `lrc::active_index` testés. En ligne **désactivé par défaut** (`lyrics_online`). Preuve : `live_lrclib_lookup` (titre public synthétique). |
| B6 | implémentée (partielle) | Musique : titres suivants de la playlist en cours, publiés en `Reading::Estimated` (« À suivre » n'est pas scriptable). Spotify et navigateurs : `NotSupported`. La file MediaRemote par lecteur (`MRMediaRemoteRequestNowPlayingPlaybackQueueForPlayerSync`, chez Vorssaint) n'est pas portée. |
| B7 | **bloquée** (choix vie privée) | Les vrais niveaux exigent un process tap Core Audio et l'autorisation « Enregistrement audio du système » (`NSAudioCaptureUsageDescription`), qui capte tout l'audio du Mac. Jugé trop intrusif pour un égaliseur : `levels` vide, `levels_measured = false`, et l'UI anime à partir de `playing`. |
| B8 | à faire (partielle) | Une pochette publiée par le navigateur via MediaRemote est affichée ; la recherche de pochette en ligne (`browser_artwork`, désactivée par défaut) n'est pas implémentée. |
| B9 | implémentée (partielle) | `include_other_players` filtre les lecteurs non musicaux (liste connue). `preferred_player` n'est **pas** honoré : l'adaptateur suit le lecteur courant du système ; l'énumération par lecteur (`MRMediaRemoteGetNowPlayingClients` + `MRPlayerPath`) reste à porter. |
| B10 | implémentée | Changement de piste → alerte compacte `AlertKind::TrackChanged` (pas pour la première piste), TTL 3 s, priorité Low. Test : `track_change_bumps_generation_and_drops_stale_results`. |

## C. Calendrier, notifications

| ID | Statut | Implémentation / preuve |
|---|---|---|
| C1 | implémentée — non testé : permission | EventKit (`objc2-event-kit`) : accès complet demandé au premier démarrage du module ou sur `CalendarCommand::RequestAccess` ; s'il est refusé, le volet Confidentialité s'ouvre au lieu d'une nouvelle invite. Écoute `EKEventStoreChangedNotification` (anti-rebond 0,5 s) + relecture toutes les 15 min. Calendriers masqués par id ou titre, événements annulés exclus, rappels datés optionnels (permission séparée). `live_eventkit_read` : « non testé : permission ». |
| C2 | implémentée (données) | `start`/`end` exposés ; compte à rebours / temps restant calculés par l'UI. |
| C3 | implémentée | Détection Meet/Zoom/Teams/FaceTime/Webex (+ Whereby, Jitsi, GoTo) dans l'URL, le lieu puis les notes ; domaines sosies rejetés ; `JoinCall` ouvre le lien. Tests `links::*` sur des chaînes synthétiques. |
| C4 | implémentée | Alerte `CalendarSoon` (priorité High) N minutes avant, une fois par occurrence (`id@start`), avec le service visio dans le sous-titre. Tests `alerts_fire_once_in_window`. |
| C5 | implémentée — non testé : permission | Bannières `com.apple.notificationcenterui` via AXObserver (source sur la run loop principale) et parcours bornés (profondeur 10, 384 nœuds, délai AX 0,1 s) ; le centre ouvert n'est jamais importé. Module désactivé par défaut ; invite Accessibilité uniquement sur `RequestAccess`. Éléments en mémoire seulement, jamais journalisés ; exclusions par nom ou bundle id, `hide_body`, `max_items`. Ouvrir = AXPress sur la bannière, sinon activation de l'app. Parcours testé sur des arbres synthétiques. **« Masquer les natives »** : évalué, non implémenté. Vorssaint déplace hors écran la fenêtre du centre de notifications (macOS 27), ce qui peut aussi cacher des alertes persistantes que l'îlot n'a pas reprises ; à reprendre après validation physique. |

## E. Accessoires, téléchargements, fichiers, caméra, raccourcis, contrôles

| ID | Statut | Implémentation / preuve |
|---|---|---|
| E1 | implémentée — non testée (aucun accessoire connecté) | Lectures dans le processus, toutes les 5 s, sans `system_profiler` : IORegistry `BatteryPercent` (HID, sans permission) + IOBluetooth (connexion ; AirPods G/D/boîtier via `batteryPercentLeft/Right/Case/Single`, présents sur macOS 27, `unsigned char`, vérifié). Alertes connexion / batterie faible après une base silencieuse, une par épisode (réarmement à seuil + 5 %). Bluetooth : invite au plus une fois (premier démarrage), ensuite IOBluetooth seulement si la permission est accordée. |
| E3 | **testée** (simulation) | FSEvents sur les dossiers réglés (par défaut ~/Downloads), rescan chaque seconde seulement pendant un téléchargement, sinon toutes les 30 s. `.crdownload/.download/.part/.opdownload/.partial`, total via Info.plist Safari ou NSProgress publié par le navigateur (jamais inventé), débit lissé, terminé ou annulé, `com.apple.DownloadFileFinished`, alerte `DownloadFinished`. Preuve : `live_fsevents_simulated_download` (fichier `.crdownload` dans un dossier temporaire → en cours → renommé → terminé, 5000 octets, alerte). Navigateur réel non testé. `DownloadsState.access` signale un dossier protégé illisible. |
| E4 | implémentée ; compression **testée** | Références seulement, en mémoire ; éléments disparus retirés ; copier les chemins ; AirDrop (`NSSharingService`) ; zip via `ditto` ponctuel (un élément) ou `zip` (plusieurs, même dossier), tué à l'arrêt du module ou après 10 min. Preuve : `compresses_synthetic_files`. Glisser-déposer : côté UI (`FileCommand::Add`). |
| E5 | implémentée — non testé : permission + UI | AVCaptureSession créée et démarrée uniquement sur `CameraCommand::Start` ; arrêtée et entrées retirées sur `Stop` (voyant éteint). Couche d'aperçu attachée à l'hôte de l'UI (`nox_core::model::camera::set_preview_host`), miroir, choix de caméra. Énumération testée (2 caméras, `live_devices_and_status`). |
| E6 | implémentée ; `shortcuts list` **testé** | Verrouillage (`SACLockScreenImmediate`), mode sombre (SkyLight), Screenshot.app (Nox n'a pas besoin de la permission Enregistrement d'écran), économiseur, corbeille (Finder via `osascript` ponctuel et tuable, hors main thread ; Automatisation ; confirmation côté UI), ouvrir app/URL (sans `file:`), `shortcuts list|run -- <nom>` ponctuels et bornés. Symboles privés résolus (`live_symbols_resolve`) ; verrouillage et bascule d'apparence non exécutés (effet visible). Concentration/NPD : **bloquée**, aucune API publique ; un raccourci utilisateur reste possible. |
| E7 | **testée** (partielle) | CoreAudio : volume et sourdine de la sortie, sourdine du micro, avec écouteurs de propriétés, sans scrutation. Luminosité de l'écran intégré via DisplayServices. Preuve : `live_controls` lit volume, sourdine, micro et luminosité, et réapplique le même volume. Rétroéclairage clavier et source d'entrée : non implémentés (CoreBrightness `KeyboardBrightnessClient` privé ; TIS pour les sources). |
| E8 | évaluation | HUD volume/luminosité dans l'îlot : afficher sans supprimer le HUD système est faisable grâce aux écouteurs CoreAudio d'E7 (alerte sur changement). Remplacer le HUD système exige un `CGEventTap` sur les touches média (Accessibilité/Surveillance de l'entrée) et la suppression des événements : intrusif, non implémenté. |
| E9 | évaluation | Mixeur par app : process taps Core Audio (macOS 14.2+) + `NSAudioCaptureUsageDescription` (capture de l'audio de chaque app). Même objection qu'en B7 ; hors priorité, utilitaire séparé conseillé. |
| E10 | évaluation | Presse-papiers : pas de notification de changement, il faut scruter `NSPasteboard.changeCount` (~0,5 s). Contenu très sensible (mots de passe) ; il faudrait exclure `org.nspasteboard.ConcealedType`/`TransientType` et rester en mémoire seulement. Faisable mais pas raisonnable sans section ni modèle dédiés (coordinateur). |
| E11 | évaluation | Captures récentes : dossier `com.apple.screencapture location` (défaut Bureau) suivi par FSEvents (déjà disponible, `sys::fsevents`). Faisable rapidement ; il faut une section, un modèle et une commande du coordinateur. Accès Bureau/Documents protégés par TCC. |
| E12 | évaluation | Bloc-notes : fichier local dans `app_support_dir()`, écriture atomique. Trivial côté données ; essentiellement de l'UI. |

## Permissions et entitlements

| Fonction | Permission | Clé Info.plist / entitlement | Quand l'invite apparaît |
|---|---|---|---|
| Calendrier | Calendriers (accès complet), Rappels si activés | `NSCalendarsFullAccessUsageDescription`, `NSRemindersFullAccessUsageDescription`, `com.apple.security.personal-information.calendars` (présents) | premier démarrage du module ou `RequestAccess` |
| Caméra | Caméra | `NSCameraUsageDescription`, `com.apple.security.device.camera` (présents) | premier `Start` ou `RequestAccess` |
| Shuffle/repeat/file, corbeille | Automatisation (Musique, Spotify, Finder) | `NSAppleEventsUsageDescription`, `com.apple.security.automation.apple-events` (présents) | action de l'utilisateur seulement |
| Accessoires | Bluetooth | `NSBluetoothAlwaysUsageDescription` (présente) | une fois au premier démarrage |
| Notifications | Accessibilité | — (pas de clé) | `RequestAccess` seulement |
| Téléchargements | Dossier Téléchargements | `NSDownloadsFolderUsageDescription` (**à ajouter**, demandé à main) | premier accès au dossier |
| Paroles en ligne | réseau | — (app non sandboxée) | option off par défaut |

## Processus externes

* `/usr/bin/perl` (adaptateur Now Playing) : un seul processus long tant que le module Musique tourne. Il sort quand stdin est fermé (SIGKILL après 1 s), et ses redémarrages sont bornés (3).
* Ponctuels, à la demande, bornés et tués à l'arrêt : `/usr/bin/ditto`, `/usr/bin/zip` (compression, noms passés en `./nom` contre l'injection d'options), `/usr/bin/shortcuts` (`list` / `run --`) et `/usr/bin/osascript` (vider la corbeille).
* perl démarre avec un environnement vidé (HOME, TMPDIR, USER, LOGNAME et PATH système seulement). `NOX_NOWPLAYING_DIR` n'est lu qu'en debug. Plafond absolu de 10 relances par démarrage du module.
* Aucun processus en boucle (pas de `system_profiler`, `pmset`, `osascript`).

## Fil principal

Toutes les interactions AppKit / NSAppleScript / AVCapture preview / NSWorkspace / AXObserver passent
par `nox_integrations::sys::on_main(timeout, f)` (alias de `nox_core::main_thread::run_with_timeout`) :
dispatch asynchrone plus attente bornée, ou exécution directe si l'appel est déjà sur le main thread. Un worker ne peut donc pas figer la sortie
de l'app (`on_quit` joint les modules sur le main thread). Le délai borne l'attente de l'appelant, pas le travail sur le main thread : seuls des appels courts y passent (AppleScript toujours `with timeout of 3 seconds`) ; ce qui est long passe par un processus ponctuel.

## Tests réels (`#[ignore]`) exécutés sur cette machine

`live_adapter_starts_answers_and_stops`, `live_lrclib_lookup`, `live_fsevents_simulated_download`,
`live_controls`, `live_shortcuts_list`, `live_symbols_resolve`, `live_devices_and_status`,
`live_hid_batteries`, `live_eventkit_read` (« non testé : permission »), `live_trust_and_center`
(terminal non autorisé).
