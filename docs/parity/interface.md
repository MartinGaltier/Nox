# Parité — Interface (A1–A20, sections, fenêtres)

Référence : Vorssaint `e916961` (voir docs/REFERENCE.md). Statuts : **à faire** · **en cours** ·
**implémentée** (code + tests automatiques) · **testée** (vérifiée sur le MacBook Air M2 réel, preuve notée) ·
**bloquée** (raison).

## Architecture de `nox-ui`

* **Couche pure (testée sans AppKit)** : `geometry` (encoche, tailles, silhouette, hit-test),
  `spring` (ressort), `machine` (compact/déplié, survol + délais), `alerts` (file à priorité + TTL),
  `compact` (activité prioritaire), `gesture` (balayage/défilement), `island` (modèle complet),
  `views/*` (chaque section produit une `Scene` : opérations de dessin + zones cliquables),
  `format` (français, valeurs `Reading` honnêtes).
* **Couche AppKit (`src/appkit`)** : panneau non activant, peintre (NSBezierPath, NSString, SF Symbols,
  pochettes), menu de barre, fenêtres Réglages et Moniteur, overlay de sélection de zone, raccourci global
  Carbon, ouverture à la connexion (SMAppService), hôte de calque caméra.
* **Zéro réveil au repos** : le minuteur d’animation (120 Hz) n’existe que tant qu’un ressort n’est pas posé ;
  les autres minuteurs sont des one-shot (délai de survol/fermeture, TTL d’alerte, prochaine seconde d’un
  compte à rebours visible). Aucun minuteur quand l’îlot est compact sans activité.
* **Données d’aperçu** : `NOX_UI_FIXTURE=1` (builds debug uniquement, module `#[cfg(debug_assertions)]`),
  ne remplit que les emplacements qu’aucun module ne publie, chaque texte est marqué « [aperçu] ».
  `NOX_UI_FORCE=expanded:<section>|compact` (debug uniquement) force l’état pour les captures.

## Lignes A

| ID | Fonction | Implémentation Nox | Vérification automatique | Statut |
|---|---|---|---|---|
| A1 | Fenêtre îlot dans l’encoche | `NSPanel` borderless + `NonactivatingPanel`, niveau `NSStatusWindowLevel + 1`, `canJoinAllSpaces | stationary | fullScreenAuxiliary | ignoresCycle`, fond transparent, silhouette noire (épaules + coins, adaptée de `NotchShape`), ombre uniquement déplié ; géométrie via `safeAreaInsets` + `auxiliaryTopLeft/RightArea` | `geometry::tests::notched_display_centres_on_camera` (M2 : 185×32 centré, collé au bord haut) | implémentée — géométrie vérifiée sur le M2 réel (`NSScreen` 1280×832 pt, encoche 562→718, panneau 516→764 × 804→832, collé au bord) ; superposition physique non capturable (voir Limites) |
| A2 | États compact / déplié | `machine::Machine` + `island::IslandModel::decide` (repos, bande d’activité, alerte, page) | `machine::tests::*`, `island::tests::hover_expands_*` | implémentée |
| A3 | Survol, délai, clic | Zone de suivi limitée à la forme (`island_contains`), `hover_delay_ms`, `close_delay_ms`, clic = ouvrir ; réglages appliqués à chaud (`Machine::configure`) | `machine::tests::*` | implémentée |
| A4 | Animations | Ressort Rust (`spring::IslandMotion`), minuteur actif seulement pendant l’animation ; fondu contenu compact ↔ page | `spring::tests::*`, `island::tests::rest_is_exactly_the_camera…` | testée (coût au repos) : build release, modules coupés : 0,00 s CPU sur 120 s et 0 réveil (`ps`/`top`) ; îlot forcé déplié : 0,00 s sur 60 s ; avec modules réels : 0,17 s/120 s (collecteur metrics) |
| A5 | Aucun vol de focus | `canBecomeKeyWindow`/`canBecomeMainWindow` = NO, panneau non activant, `acceptsFirstMouse` | propriétés lues dans l’app réelle : `canBecomeKey=false canBecomeMain=false isKey=false nonactivating=true`, app jamais active, app au premier plan inchangée à l’ouverture | implémentée — clic physique non testé (pas d’entrée synthétique) |
| A6 | Écrans sans encoche | Capsule dans la barre ou encoche suspendue (`IslandSettings.silhouette`), rien au repos si `simulate_notch` = false ; choix d’écran `DisplayChoice` | `geometry::tests::external_display_simulates_or_hides` | implémentée |
| A7 | Changement d’écran | `NSApplicationDidChangeScreenParametersNotification` → nouvelle géométrie, sans animation entre écrans | — | implémentée |
| A8 | Plein écran | Liste des fenêtres de l’app au premier plan (sans titres : aucune permission) ; masqué si une fenêtre couvre tout l’écran de l’îlot ; recalcul sur changement d’espace / d’app active (pas de sondage) | `screens::tests::coverage_rules` | implémentée |
| A9 | Sections activables / réorganisables | Réglages › Sections : case « afficher », case « module actif » (arrêt réel via `on_settings_changed`), ▲▼ | `views::tests`, `island::tests::section_paging_skips_disabled_modules` | implémentée |
| A10 | Gestes | Balayage horizontal = section suivante/précédente (adapté de `NotchGestureSupport`), tirer vers le bas = ouvrir, pousser = fermer, défilement vertical dans les listes, haptique `NSHapticFeedbackManager` | `gesture::tests::*`, `island::tests::list_scroll_is_clamped` | implémentée |
| A11 | Alertes temporaires | `hub.drain_alerts` → `AlertQueue` (priorité, préemption, TTL), notice sous l’encoche, clic = ouvrir la section liée | `alerts::tests::*`, `island::tests::alert_shows_notice…` | implémentée |
| A12 | Barre des menus | `NSStatusItem` : ouvrir l’îlot, état + temps restant du maintien éveillé, modes × durées (15/30/60/120/∞), arrêt, assistant capot fermé, moniteur, réglages, quitter (`on_quit` puis `terminate:`) ; menu reconstruit à chaque ouverture | — | implémentée |
| A13 | Réglages | Fenêtre AppKit native (onglets Général, Sections, Contenus, Agents & surveillance, Système & énergie) ; `store.save` puis `on_settings_changed` | — | implémentée |
| A14 | Arrêt des collectes à la désactivation | Case « module actif » → `ModuleManager` | test core `disabling_a_module_stops_it` | implémentée (core) |
| A15 | Masqué dans les captures | `sharingType = none` si `hide_in_captures` (et pendant une surveillance de zone) | — | implémentée |
| A16 | Masquer jusqu’au survol | `hide_until_hover` : rien n’est dessiné au repos tant que le pointeur n’est pas sur l’îlot | — | implémentée |
| A17 | Taille / silhouette / contour / translucide | `size` (compacte 440 / spacieuse 520 / personnalisée 360–640), `silhouette`, `outline`, `translucent` | `geometry::tests::expanded_size_respects_presets_and_screen` | implémentée |
| A18 | Écran verrouillé | Nécessite une API privée SkyLight pour une fenêtre au-dessus de l’écran de verrouillage | — | bloquée (API privée non retenue) |
| A19 | Raccourci global | ⌃⌥N via Carbon `RegisterEventHotKey` (aucune permission Accessibilité), option `shortcut_enabled` | — | implémentée |
| A20 | Contenu au repos | `idle_content` : rien / batterie / musique en pause / agents | `compact::tests::*` | implémentée |

## Rendu des sections (îlot déplié)

| Section | Contenu | Absence de données |
|---|---|---|
| Musique | pochette, titre/artiste/album, lecteur (clic = ouvrir), progression interpolée (`elapsed_now`) + seek, précédent/lecture/suivant, aléatoire/répétition si exposés, paroles synchronisées (ligne active) ou file, égaliseur (marqué « non mesuré » si animé) | module absent / « Rien en lecture » / raison des paroles ou de la file |
| Calendrier | événements N jours, groupés par jour, horaire, lieu, « dans 10 min », Rejoindre (visio), clic = ouvrir | autorisation (bouton Autoriser ou Réglages Système) / « Aucun événement » |
| Agents IA | carte par fournisseur détecté : sessions (phase, projet, outil, modèle), limites + reset, jetons du jour, coût « ≈ … $ (estimé) » | « Aucun agent détecté », limites désactivées (opt-in) |
| Minuteurs | minuteurs (pause/reprise/+1 min/annuler, préréglages), chronomètre (tours), Pomodoro | état vide par colonne |
| Notifications | app, titre, corps (masquable), heure relative, effacer, tout effacer | autorisation Accessibilité / vide |
| Téléchargements | progression (ou indéterminée), octets, débit, temps restant (estimé), source, afficher/ouvrir/retirer | vide |
| Fichiers | étagère (cible de glisser-déposer → `FileCommand::Add`), AirDrop, compresser, copier les chemins, vider, ajouter… | zone de dépôt |
| Système | CPU, GPU, mémoire + pression, batterie, réseau, puissance + mini-graphes à trous ; `MetricsVisible(true/false)` selon la visibilité | « Indisponible » / raison, jamais 0 |
| Maintien éveillé | état + temps restant, 3 modes × 5 durées, arrêt, assistant capot fermé (explication puis `InstallHelper`) | module absent |
| Accessoires | appareils connectés, batteries G/D/boîtier | vide / raison |
| Surveillance | phase, cible, taille, dernier changement, choix du déclencheur → overlay de sélection → `WatchCommand::Start` | autorisation Enregistrement d’écran |
| Caméra | calque hôte remis via `set_preview_host` (hôte puis Start ; Stop puis None), choix de caméra, erreur du module | autorisation / erreur (« caméra occupée »…) ; aperçu réel non testé (pas d’accès caméra pour l’agent) |
| Raccourcis | contrôles volume/muet/luminosité/micro (RefreshControls à l’ouverture), actions rapides (corbeille confirmée, pas de « Ne pas déranger »), raccourcis Raccourcis.app, dernier résultat | contrôle absent grisé avec la raison |
| Captures | vignettes réduites (jamais l’image pleine taille gardée en mémoire), ouvrir / afficher / copier l’image / corbeille, Refresh à l’ouverture | dossier refusé (Fichiers et dossiers) / « Aucune capture récente » |
| Bloc-notes | NSTextView natif ; le panneau ne peut devenir clé que pendant l’édition et rend le clavier à l’app au premier plan ensuite ; SetText à chaque modification, texte jamais journalisé, Effacer confirmé | erreur d’écriture / texte tronqué affichés |
| Presse-papiers | liste (aperçu, app source, taille), restaurer / retirer / tout effacer ; mémoire seulement | accès au collage non déterminé (bouton Autoriser → une lecture, invite macOS une fois) / refusé (explication) / vide |
| Accessoires (Bluetooth) | bouton « Autoriser le Bluetooth » si non déterminé, « Ouvrir les Réglages Système » si refusé ; batteries IORegistry affichées sans autorisation | — |
| Musique (sources) | sélecteur de lecteur quand plusieurs publient (lecteur préféré enregistré), `MusicCommand::Visible` à l’ouverture/fermeture de la page, réglages pochettes en ligne et niveaux audio réels, purge des caches | — |
| Mixeur (E9) | liste des apps audio (nom, « joue », « muet », « réglage actif » quand un tap est en place), curseur 0–200 % (clic) → `SetVolume`, muet → `SetMuted`, réinitialiser par app → `Reset`, « Tout réinitialiser » → `ResetAll` ; liste défilante ; `MixerCommand::Visible` à l’ouverture/fermeture de la page (repli compris) | module désactivé / en attente / raison complète de `unavailable` / autorisation refusée : explication et lien vers Réglages Système (confidentialité, enregistrement audio du système), contrôles masqués. **Non testée avec un vrai tap** (invite système réservée à l’utilisateur) : tests unitaires `views::mixer::tests` seulement. Réglages persistés par bundle id (`settings.mixer`, `remember`) ; Réglages › Musique : note « niveaux réels indisponibles » + lien quand `levels_refused` |

Fenêtre **Moniteur système** : cœurs P/E, GPU, capteurs, mémoire/pression/swap, batterie (santé, cycles,
autonomie, flux), puissance, réseau (↓/↑ + historique), disques, ventilateurs — historiques bornés avec trous.

## Revue croisée (activities, d856546) traitée

* M1 overlay : l’app au premier plan est réactivée après le tracé ou Échap ; M4 : overlay `sharingType = none`.
* M2/M3 : rendu via `LimitWindow::status` — reset estimé « ≈ … (estimé) », fenêtre réinitialisée « réinitialisée — en attente d’une nouvelle lecture », sans rafraîchissement.
* F1 cycle Rc rompu à la fermeture de l’overlay, F2 curseur restauré, F3 plus de SetTrigger, F6 raisons complètes sur deux lignes, F7 tarifs/comptes/budget/forfait, F8 chevauchement corrigé.
* F4/F11 : redessin différentiel (la scène est comparée à la précédente ; rien n’est invalidé si elle est identique, sinon seulement le rectangle modifié). Musique en lecture, égaliseur animé : 0,69 s CPU / 60 s en build debug.
* F9/F10 : `Timer::new` exige le thread principal ; les observateurs passent par `NSOperationQueue.mainQueue`.

## Coordination

* Caméra : hook `set_preview_host(Option<usize>)` à fournir par `integrations` dans `model/camera.rs` ;
  côté UI `appkit/camera.rs` (calque hôte retenu par l’UI, main thread uniquement).
* Surveillance : overlay côté UI ; `BeginSelection` → overlay → `Start{region, trigger}` / `Stop`
  (passera à `CancelSelection` quand le modèle d’`activities` sera sur main).
* Modèles annoncés mais pas encore sur main (adaptation à l’intégration) : `PowerState` (Toggle,
  `helper_busy`, `lid_sleep_disabled: Reading<bool>`, `ClientRejected`), `MusicState` (`unavailable`,
  `automation`, `RequestAutomation`, `lyrics_online`), `CompanionState` (intensity, reaction…), `WatchState`.

## Icône

`packaging/AppIcon.icns` est originale : générée par `crates/nox-ui/assets/make_icon.py`
(`python3 -I make_icon.py icon_1024.png`, puis `sips -z` aux tailles d’un `.iconset` et `iconutil -c icns`).

## Vérification réelle (MacBook Air M2, macOS 27.0)

* Rendu : captures internes debug (`NOX_UI_SNAPSHOT`, `cacheDisplayInRect`) de la bande compacte (musique) et de toutes les pages dépliées, du moniteur avec les **vraies** mesures du module metrics (8 cœurs E/P, santé batterie « estimé », puissances CPU/ANE « Indisponible », capteurs, disque) et du maintien éveillé avec le vrai module power.
* Réglages : rapport de mise en page (5 onglets, 152 contrôles, aucun hors de la zone visible de 714 pt).
* Release : aucune chaîne de fixture ni de hook debug dans le binaire (`strings`).
* **Avec tous les vrais modules** (main c0aa292 fusionné) : bande compacte « agent qui travaille » pilotée par la session Claude réelle, page Agents (forfaits Pro/Plus, coûts « estimé », fenêtre réinitialisée affichée « … »), Raccourcis avec les vrais niveaux volume/luminosité/muet, Calendrier en attente d’autorisation (bouton Autoriser), Musique « Rien en lecture », Système/Moniteur avec les mesures M2. Réglages : 6 onglets, aucun contrôle hors champ.
* CPU, build release, vrais modules (accessoires coupés), îlot compact au repos : 0,52 s sur 120 s (≈ 0,4 %, threads des modules : agents qui lisent les journaux de la session en cours, metrics) ; échantillon `sample` : thread principal (UI) 100 % en attente (`mach_msg`).

## Vérification finale (build release, réglages par défaut, accessoires actifs)

| Configuration | Thread principal (`sample`) | CPU au repos (`ps`) | RSS | Threads |
|---|---|---|---|---|
| Réglages par défaut | inactif dans 1365/1369 échantillons, aucun gel | 1,70 s / 120 s (≈ 1,4 %) | 55 Mo | 19 |
| Défaut, module agents coupé | — | 1,27 s / 120 s (≈ 1,1 %) | 55 Mo | 18 |
| Interface seule (tous modules coupés) | inactif | 0,00 s / 120 s, 0 réveil | — | — |

Le coût « au repos » mesuré avec les modules ne vient pas de l’interface : il vient des threads des modules. Pendant ces mesures, le module agents relit les journaux que la session Claude de l’agent écrit en continu : ce n’est pas un vrai repos. Le gel IOBluetooth (accessoires) est corrigé sur main (29ae247) et ne se reproduit plus.

## Limites

* Gel IOBluetooth au lancement (module accessoires) : signalé, corrigé par integrations (29ae247), revérifié en release.
* `screencapture` échoue (« could not create image from display ») : le processus de l’agent n’a pas l’autorisation Enregistrement de l’écran. Les captures internes ne montrent pas l’îlot superposé à la vraie encoche : à valider à l’œil.
* Aucune entrée synthétique possible (Accessibilité non accordée) : survol, clic, balayage, glisser-déposer et overlay de sélection ne sont couverts que par les tests unitaires.
* Le texte des contrôles standard de la fenêtre Réglages n’est pas rendu hors écran : vérifiée par rapport de mise en page, pas visuellement.

* A18 (écran verrouillé) non implémenté : demande une API privée.
* La détection plein écran assimile une fenêtre « zoomée » sous une barre des menus masquée à un plein écran.
* Mises à jour d’équaliseur animé à 10 Hz pendant la lecture uniquement (pas de niveaux réels : aucune
  permission d’enregistrement audio demandée).
